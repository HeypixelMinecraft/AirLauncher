/*
 * AirLauncher
 * A Tauri-based launcher for the AirPlus Minecraft client.
 * https://github.com/lmx0721/AirPlus
 *
 * 统一下载管线：并发限制、Range 断点续传、sha1 校验、指数退避重试、节流进度事件。
 */
use crate::error::{LauncherError, Result};
use crate::events;
use crate::manifest;
use crate::net;
use crate::paths;
use crate::state::AppState;
use futures_util::StreamExt;
use serde::Serialize;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

static SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();
static TASK_SEQ: AtomicU64 = AtomicU64::new(1);
/// 目标文件 -> 正在写它的任务：同一路径并发下载会互相覆盖 .part，必须去重
static ACTIVE_DESTS: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();

fn active_dests() -> &'static Mutex<HashMap<PathBuf, String>> {
    ACTIVE_DESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

const MAX_ATTEMPTS: u32 = 3;

/// 超过这个尺寸才考虑分片下载（小文件建连成本占主导，分片反而更慢）
const SEGMENT_THRESHOLD: u64 = 4 * 1024 * 1024;
/// 每片最小字节数，避免切出一堆只有几百 KB 的碎片
const MIN_SEGMENT_SIZE: u64 = 1024 * 1024;
/// 分片数上限（再多连接收益递减，还容易被 CDN 限流）
const MAX_SEGMENTS: u64 = 8;

/// 全局并发上限（兜底）。实际并发由调用方控制（设置里的 download_concurrency
/// 上限 16），这里只防止异常路径把连接数顶爆。
fn semaphore() -> &'static Semaphore {
    SEMAPHORE.get_or_init(|| Semaphore::new(16))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub task_id: String,
    pub name: String,
    pub downloaded: u64,
    pub total: u64,
    pub speed_bps: u64,
    /// running | done | error | cancelled
    pub state: String,
    pub error: Option<String>,
}

/// 下载结果的完整信息。sha1 一律在流式写入后顺带算出，
/// 供下载清单（manifest）落盘，调用方不必再读一遍文件。
pub struct Downloaded {
    pub path: PathBuf,
    pub size: u64,
    pub sha1: String,
}

pub fn new_task_id(prefix: &str) -> String {
    format!("{}-{}", prefix, TASK_SEQ.fetch_add(1, Ordering::Relaxed))
}

/// 进度聚合器：各任务只把**最新值**写进来，由 `spawn_progress_flusher`
/// 每 100ms 汇总成一个数组 emit 一次。
///
/// 以前每条进度都单独 emit：8 并发 × 100ms = 80 events/s，
/// 每个事件都要过一遍 IPC + Svelte 失效 + TaskPanel diff，是切窗口卡顿的主因之一。
/// 现在无论多少并发，事件频率恒定 10/s，同一任务在窗口期内的多次更新自然去重。
struct ProgressHub {
    dirty: Mutex<HashMap<String, DownloadProgress>>,
}

static HUB: OnceLock<ProgressHub> = OnceLock::new();

fn hub() -> &'static ProgressHub {
    HUB.get_or_init(|| ProgressHub {
        dirty: Mutex::new(HashMap::new()),
    })
}

fn push(p: DownloadProgress) {
    hub()
        .dirty
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(p.task_id.clone(), p);
}

fn emit(_app: &AppHandle, p: &DownloadProgress) {
    push(p.clone());
}

/// 供上层模块上报「聚合进度」（例如资源包整体进度），走同一个 100ms 合批通道
pub fn report_progress(p: DownloadProgress) {
    push(p);
}

/// 启动进度汇总任务（在 lib.rs 的 setup 里调用一次）
pub fn spawn_progress_flusher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(100));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let batch: Vec<DownloadProgress> = {
                let mut map = hub().dirty.lock().unwrap_or_else(|e| e.into_inner());
                if map.is_empty() {
                    continue;
                }
                map.drain().map(|(_, v)| v).collect()
            };
            let _ = app.emit(events::DOWNLOAD_PROGRESS, batch);
        }
    });
}

/// 启动一个后台下载任务（立即返回 task_id，进度走 download://progress 事件）。
///
/// `origin` 非空时，下载成功后会往下载清单里写一条记录（客户端 / 模组 + sha1）。
pub fn start_task(
    app: &AppHandle,
    name: &str,
    url: &str,
    dest: PathBuf,
    sha1: Option<String>,
    origin: Option<manifest::Origin>,
) -> String {
    // 同一目标文件已有任务在跑：直接复用，避免连点产生多个任务把同一个 .part 写坏
    if let Some(existing) = active_dests()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&dest)
        .cloned()
    {
        return existing;
    }

    let task_id = new_task_id("dl");
    let token = CancellationToken::new();

    {
        let state = app.state::<AppState>();
        state
            .cancels
            .lock()
            .unwrap()
            .insert(task_id.clone(), token.clone());
    }

    active_dests()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(dest.clone(), task_id.clone());

    // 立刻推一条「0 字节」的进度：前端点完下载马上就能看到任务卡片，
    // 不用等网络探速/首包到达（GitHub 慢的时候这段可能有好几秒）。
    push(DownloadProgress {
        task_id: task_id.clone(),
        name: name.to_string(),
        downloaded: 0,
        total: 0,
        speed_bps: 0,
        state: "running".into(),
        error: None,
    });

    let app = app.clone();
    let name = name.to_string();
    let url = url.to_string();
    let task_id_inner = task_id.clone();
    let dest_for_map = dest.clone();
    let file_name = dest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    tauri::async_runtime::spawn(async move {
        let task_id = task_id_inner;
        let result = run_download_full(
            &app,
            Some(&task_id),
            &name,
            &url,
            &dest,
            sha1.as_deref(),
            &token,
        )
        .await;

        // 下载成功才记录，失败/取消不留痕迹
        if let (Ok(done), Some(o)) = (&result, origin.as_ref()) {
            manifest::record_download(o, &file_name, &url, done.size, done.sha1.clone());

            // 客户端 jar 一落地就收敛成唯一生效版本：只靠前端拿完成事件再调
            // client_activate 的话，中间这段时间 mods 里会同时躺着新旧两个客户端，
            // Forge 会加载先枚举到的旧 jar（「下了 b4 跑的是 b3」就是这么来的）。
            if matches!(o, manifest::Origin::Client { .. }) {
                let mods = paths::get().mods_dir.clone();
                let name = file_name.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    crate::client::adopt(&mods, &name)
                })
                .await;
            }
        }

        // 终态进度由 run_download_full 统一发（内部下载与后台任务共用同一套收尾）

        app.state::<AppState>()
            .cancels
            .lock()
            .unwrap()
            .remove(&task_id);
        active_dests()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&dest_for_map);
    });

    task_id
}

pub fn cancel_task(app: &AppHandle, task_id: &str) {
    if let Some(token) = app.state::<AppState>().cancels.lock().unwrap().get(task_id) {
        token.cancel();
    }
}

/// 完整重试循环：网络错误保留 .part 以便续传，sha1 错误清空重来。
///
/// `task_id` 为 None 时不发单文件进度事件——批量下载（如 resources 的上千个
/// 资源对象）由调用方汇总成一个整体进度上报，避免 IPC 与列表 diff 爆炸。
pub async fn run_download(
    app: &AppHandle,
    task_id: Option<&str>,
    name: &str,
    url: &str,
    dest: &Path,
    sha1_hex: Option<&str>,
    token: &CancellationToken,
) -> Result<PathBuf> {
    Ok(
        run_download_full(app, task_id, name, url, dest, sha1_hex, token)
            .await?
            .path,
    )
}

/// 同 `run_download`，但把大小与 sha1 一并返回（写下载清单用）。
///
/// 只要调用方给了 `task_id`，这里就保证「发过 running 进度」之后一定发一条终态。
/// 否则那些在启动流程里被 await 的内部下载（Forge / 客户端 / JRE / 库）会永远停在
/// 最后一条 running 进度上——游戏都起来了，任务面板还挂着「下载中」和取消按钮。
pub async fn run_download_full(
    app: &AppHandle,
    task_id: Option<&str>,
    name: &str,
    url: &str,
    dest: &Path,
    sha1_hex: Option<&str>,
    token: &CancellationToken,
) -> Result<Downloaded> {
    let result = retry_download(app, task_id, name, url, dest, sha1_hex, token).await;
    if let Some(tid) = task_id {
        emit_terminal(tid, name, &result);
    }
    result
}

/// 终态进度：done 时把进度条推满，失败/取消则标记状态，让前端 5 秒后自动移除卡片
fn emit_terminal(task_id: &str, name: &str, result: &Result<Downloaded>) {
    let p = match result {
        Ok(d) => DownloadProgress {
            task_id: task_id.to_string(),
            name: name.to_string(),
            downloaded: d.size,
            total: d.size,
            speed_bps: 0,
            state: "done".into(),
            error: None,
        },
        Err(LauncherError::Cancelled) => DownloadProgress {
            task_id: task_id.to_string(),
            name: name.to_string(),
            downloaded: 0,
            total: 0,
            speed_bps: 0,
            state: "cancelled".into(),
            error: None,
        },
        Err(e) => DownloadProgress {
            task_id: task_id.to_string(),
            name: name.to_string(),
            downloaded: 0,
            total: 0,
            speed_bps: 0,
            state: "error".into(),
            error: Some(e.to_string()),
        },
    };
    push(p);
}

/// 带重试的下载主体：网络错误保留 .part 以便续传，sha1 错误清空重来。
async fn retry_download(
    app: &AppHandle,
    task_id: Option<&str>,
    name: &str,
    url: &str,
    dest: &Path,
    sha1_hex: Option<&str>,
    token: &CancellationToken,
) -> Result<Downloaded> {
    let mut last_err: Option<LauncherError> = None;

    for attempt in 1..=MAX_ATTEMPTS {
        if token.is_cancelled() {
            return Err(LauncherError::Cancelled);
        }

        match download_once(app, task_id, name, url, dest, sha1_hex, token).await {
            Ok(p) => return Ok(p),
            Err(LauncherError::Cancelled) => return Err(LauncherError::Cancelled),
            Err(e @ LauncherError::Checksum(_)) => {
                let _ = tokio::fs::remove_file(dest).await; // 连 .part 一起不要
                let _ = tokio::fs::remove_file(dest.with_extension("part")).await;
                last_err = Some(e); // 校验失败重下也没有意义差异，但保留统一重试路径
            }
            Err(e) => {
                // 网络类失败：让下次重试重新测速，可能换一条更稳的线路
                crate::mirror::forget(url);
                last_err = Some(e);
                if attempt < MAX_ATTEMPTS {
                    tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
                }
            }
        }
    }

    Err(last_err.unwrap_or_else(|| LauncherError::Message("下载失败（未知原因）".into())))
}

async fn download_once(
    app: &AppHandle,
    task_id: Option<&str>,
    name: &str,
    url: &str,
    dest: &Path,
    sha1_hex: Option<&str>,
    token: &CancellationToken,
) -> Result<Downloaded> {
    let _permit = semaphore().acquire().await;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let part = dest.with_extension("part");
    let mut resume_from = 0u64;
    if part.exists() {
        resume_from = tokio::fs::metadata(&part).await?.len();
    }

    // GitHub 直连在国内常被限速：先并发测速挑最快线路（含直连与公共镜像），
    // 非 GitHub 地址原样返回，不影响其它下载源。
    let resolved = crate::mirror::resolve(url).await;
    if token.is_cancelled() {
        return Err(LauncherError::Cancelled);
    }

    // 大文件（客户端 jar）在全新下载时走多连接分片，能明显吃满带宽；
    // 断点续传仍走单连接（.part 里已有内容，分片拼接会破坏续传语义）。
    if resume_from == 0
        && crate::mirror::is_github(url)
        && let Some(total) = probe_total(&resolved).await
        && total >= SEGMENT_THRESHOLD
    {
        match download_segmented(app, task_id, name, &resolved, dest, sha1_hex, token, total).await
        {
            Ok(done) => return Ok(done),
            Err(LauncherError::Cancelled) => return Err(LauncherError::Cancelled),
            // 镜像不支持 Range / 分片中途失败：静默回退单连接，
            // 不走重试循环，避免「明明能下却报分片失败」
            Err(_) => {}
        }
        if token.is_cancelled() {
            return Err(LauncherError::Cancelled);
        }
    }

    let mut req = net::get()
        .get(&resolved)
        // 明确要求不压缩：保证 Content-Length 与 Range 语义可靠
        .header("Accept-Encoding", "identity");
    if resume_from > 0 {
        req = req.header("Range", format!("bytes={resume_from}-"));
    }
    let resp = req.send().await?.error_for_status()?;
    let resumed = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let total: u64 = resp
        .content_length()
        .map(|len| len + if resumed { resume_from } else { 0 })
        .unwrap_or(0);

    let mut file = if resumed {
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .await?;
        f.seek(std::io::SeekFrom::End(0)).await?;
        f
    } else {
        tokio::fs::File::create(&part).await?
    };

    let mut downloaded = if resumed { resume_from } else { 0 };
    let mut last_emit = Instant::now();
    let mut last_downloaded = downloaded;
    let mut window_start = Instant::now();
    let mut window_downloaded = 0u64;

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = token
        .run_until_cancelled(stream.next())
        .await
        .ok_or(LauncherError::Cancelled)?
        .transpose()?
    {
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        window_downloaded += chunk.len() as u64;

        if last_emit.elapsed() >= Duration::from_millis(100) {
            let pct_changed = total == 0 || (downloaded - last_downloaded) * 100 / total >= 1;
            if pct_changed {
                if let Some(tid) = task_id {
                    let speed = (window_downloaded as f64
                        / window_start.elapsed().as_secs_f64().max(0.05))
                        as u64;
                    emit(
                        app,
                        &DownloadProgress {
                            task_id: tid.to_string(),
                            name: name.to_string(),
                            downloaded,
                            total,
                            speed_bps: speed,
                            state: "running".into(),
                            error: None,
                        },
                    );
                }
                last_emit = Instant::now();
                last_downloaded = downloaded;
                window_start = Instant::now();
                window_downloaded = 0;
            }
        }
    }
    file.flush().await?;
    drop(file);

    if token.is_cancelled() {
        return Err(LauncherError::Cancelled);
    }

    finish_file(part, dest, sha1_hex, downloaded).await
}

/// 校验 + 落盘（单连接与分片下载共用收尾）
async fn finish_file(
    part: PathBuf,
    dest: &Path,
    sha1_hex: Option<&str>,
    size: u64,
) -> Result<Downloaded> {
    // 无论调用方是否给了期望值都算一遍：算出来的 hash 要写进下载清单，
    // 有期望值时顺带校验。资源类下载本就带 sha1，不会多读一次文件。
    let actual = file_sha1(&part).await?;
    if let Some(expected) = sha1_hex
        && !actual.eq_ignore_ascii_case(expected)
    {
        return Err(LauncherError::Checksum(format!(
            "sha1 校验失败：期望 {expected}，实际 {actual}"
        )));
    }

    tokio::fs::rename(&part, dest).await?;
    Ok(Downloaded {
        path: dest.to_path_buf(),
        size,
        sha1: actual,
    })
}

/// 探测服务端是否支持 Range 以及文件总大小（`Range: bytes=0-0`）
///
/// 只等响应头，不读正文；加超时是为了不让慢线路把「开始下载」拖成几十秒。
async fn probe_total(url: &str) -> Option<u64> {
    tokio::time::timeout(Duration::from_secs(6), async {
        let resp = net::get()
            .get(url)
            .header("Range", "bytes=0-0")
            .header("Accept-Encoding", "identity")
            .send()
            .await
            .ok()?;
        if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return None;
        }
        let range = resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)?
            .to_str()
            .ok()?;
        // 形如 `bytes 0-0/123456`
        range.rsplit('/').next()?.trim().parse().ok()
    })
    .await
    .ok()
    .flatten()
}

/// 多连接分片下载：把文件切成 N 段并发拉取，最后拼接。
///
/// 单连接受限于「每连接限速」时（GitHub 及其镜像很常见）提升非常明显；
/// 每个分片都校验返回码与长度，任何一片失败就整体重试（.part 会被清掉）。
#[allow(clippy::too_many_arguments)]
async fn download_segmented(
    app: &AppHandle,
    task_id: Option<&str>,
    name: &str,
    url: &str,
    dest: &Path,
    sha1_hex: Option<&str>,
    token: &CancellationToken,
    total: u64,
) -> Result<Downloaded> {
    let wanted = app
        .state::<AppState>()
        .settings_snapshot()
        .download_concurrency
        .clamp(2, 16) as u64;
    let seg_count = wanted
        .min(MAX_SEGMENTS)
        .min((total / MIN_SEGMENT_SIZE).max(1));
    let chunk = total.div_ceil(seg_count);

    let progress = Arc::new(AtomicU64::new(0));

    // 进度上报：分片内部只累加原子计数，由独立任务每 100ms 汇总一次
    let ticker = task_id.map(|tid| {
        let tid = tid.to_string();
        let name = name.to_string();
        let progress = progress.clone();
        tokio::spawn(async move {
            let mut last = 0u64;
            let mut last_at = Instant::now();
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let cur = progress.load(Ordering::Relaxed);
                let speed = (cur.saturating_sub(last) as f64
                    / last_at.elapsed().as_secs_f64().max(0.05)) as u64;
                push(DownloadProgress {
                    task_id: tid.clone(),
                    name: name.clone(),
                    downloaded: cur,
                    total,
                    speed_bps: speed,
                    state: "running".into(),
                    error: None,
                });
                last = cur;
                last_at = Instant::now();
            }
        })
    });

    let mut jobs = Vec::new();
    let mut seg_paths = Vec::new();
    for i in 0..seg_count {
        let start = chunk * i;
        if start >= total {
            break;
        }
        let end = (start + chunk).min(total) - 1;
        let seg_path = dest.with_extension(format!("part.{i}"));
        seg_paths.push(seg_path.clone());
        jobs.push(tokio::spawn(fetch_segment(
            url.to_string(),
            start,
            end,
            seg_path,
            progress.clone(),
            token.clone(),
        )));
    }

    let mut failure: Option<LauncherError> = None;
    for job in jobs {
        match job.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                failure.get_or_insert(e);
            }
            Err(e) => {
                failure.get_or_insert(LauncherError::Message(format!("分片任务异常: {e}")));
            }
        }
    }
    if let Some(t) = ticker {
        t.abort();
    }

    if let Some(e) = failure {
        for p in &seg_paths {
            let _ = tokio::fs::remove_file(p).await;
        }
        return Err(e);
    }
    if token.is_cancelled() {
        for p in &seg_paths {
            let _ = tokio::fs::remove_file(p).await;
        }
        return Err(LauncherError::Cancelled);
    }

    // 按顺序拼接分片
    let part = dest.with_extension("part");
    {
        let mut out = tokio::fs::File::create(&part).await?;
        for p in &seg_paths {
            let mut f = tokio::fs::File::open(p).await?;
            tokio::io::copy(&mut f, &mut out).await?;
            let _ = tokio::fs::remove_file(p).await;
        }
        out.flush().await?;
    }

    finish_file(part, dest, sha1_hex, total).await
}

/// 下载单个分片；长度不符直接判失败，避免拼出损坏文件
async fn fetch_segment(
    url: String,
    start: u64,
    end: u64,
    path: PathBuf,
    progress: Arc<AtomicU64>,
    token: CancellationToken,
) -> Result<()> {
    if token.is_cancelled() {
        return Err(LauncherError::Cancelled);
    }

    let resp = net::get()
        .get(&url)
        .header("Range", format!("bytes={start}-{end}"))
        .header("Accept-Encoding", "identity")
        .send()
        .await?
        .error_for_status()?;
    if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(LauncherError::Message(
            "服务器不支持分片下载，已回退单连接".into(),
        ));
    }

    let mut file = tokio::fs::File::create(&path).await?;
    let mut got = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = token
        .run_until_cancelled(stream.next())
        .await
        .ok_or(LauncherError::Cancelled)?
        .transpose()?
    {
        file.write_all(&chunk).await?;
        got += chunk.len() as u64;
        progress.fetch_add(chunk.len() as u64, Ordering::Relaxed);
    }
    file.flush().await?;

    let expected = end - start + 1;
    if got != expected {
        return Err(LauncherError::Message(format!(
            "分片下载不完整（{got}/{expected}）"
        )));
    }
    Ok(())
}

/// 流式计算 sha1（同步版）：按 64KB 分块读，避免把整个 jar 读进内存。
/// 下载校验、手动导入文件的哈希、以及其它阻塞场景都用它。
pub fn sha1_file_sync(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// 流式计算 sha256（同步版）：Adoptium JRE zip 的官方 checksum 用，逻辑同 sha1。
pub fn sha256_file_sync(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// 异步包装：哈希是 CPU + 磁盘 IO 混合，丢进阻塞线程池，别占 tokio worker。
async fn file_sha1(path: &Path) -> Result<String> {
    let p = path.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || sha1_file_sync(&p))
        .await
        .map_err(|e| LauncherError::Message(format!("计算 sha1 失败: {e}")))?
}

/// 内部任务直接 await 下载完成（不产生独立 task_id）
pub async fn fetch_to(
    app: &AppHandle,
    task_id: &str,
    name: &str,
    url: &str,
    dest: &Path,
    sha1_hex: Option<&str>,
    token: &CancellationToken,
) -> Result<PathBuf> {
    run_download(app, Some(task_id), name, url, dest, sha1_hex, token).await
}

/// 同 `fetch_to`，额外拿到大小与 sha1（下载清单用）
pub async fn fetch_to_full(
    app: &AppHandle,
    task_id: &str,
    name: &str,
    url: &str,
    dest: &Path,
    sha1_hex: Option<&str>,
    token: &CancellationToken,
) -> Result<Downloaded> {
    run_download_full(app, Some(task_id), name, url, dest, sha1_hex, token).await
}

/// 静默下载：不发单文件进度（调用方自己汇总整体进度）
pub async fn fetch_to_quiet(
    app: &AppHandle,
    url: &str,
    dest: &Path,
    sha1_hex: Option<&str>,
    token: &CancellationToken,
) -> Result<PathBuf> {
    run_download(app, None, "", url, dest, sha1_hex, token).await
}

pub fn cache_path(file_name: &str) -> PathBuf {
    paths::get().cache_dir.join(file_name)
}
