/*
 * AirLauncher
 * A Tauri-based launcher for the AirPlus Minecraft client.
 * https://github.com/lmx0721/AirPlus
 *
 * 启动游戏：JRE -> Forge -> 版本链合并 -> 资源下载 -> 组装 V14 命令行 -> 拉起进程。
 * 流程进度经 launch://log 转发；下载明细复用 download://progress（task_id 前缀 launch-*）。
 */
use crate::error::{LauncherError, Result};
use crate::events;
use crate::{forge, java, mojang, paths, state::AppState};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchOptions {
    /// 离线玩家名（account_id 为空时生效）
    pub player_name: String,
    /// MSA 账号 id；Some = 正版账号（临期自动静默刷新）
    pub account_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchStatus {
    pub running: bool,
    pub pid: Option<u32>,
}

pub fn task_token(app: &AppHandle) -> CancellationToken {
    let token = CancellationToken::new();
    app.state::<AppState>()
        .cancels
        .lock()
        .unwrap()
        .insert("launch".into(), token.clone());
    token
}

/// launch://exit 的载荷：error = 启动流程失败原因；exit_code = 游戏进程退出码。
/// 正常结束 {error: null, exitCode: 0}；崩溃时 exitCode 非 0，前端据此引导看日志。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchExitPayload {
    pub error: Option<String>,
    pub exit_code: Option<i32>,
}

/// 后台执行完整启动流程
pub fn start(app: &AppHandle, options: LaunchOptions) {
    let app = app.clone();
    let token = task_token(&app);
    // 正式启动会与预热争用同一些下载目标，先把预热停掉
    crate::warmup::cancel();

    tauri::async_runtime::spawn(async move {
        let result = run(&app, options, &token).await;
        app.state::<AppState>()
            .cancels
            .lock()
            .unwrap()
            .remove("launch");

        match &result {
            Ok(_) => emit_log(&app, "启动流程完成，游戏运行中…"),
            Err(LauncherError::Cancelled) => emit_log(&app, "启动已取消"),
            Err(e) => emit_log(&app, &format!("启动失败: {e}")),
        }
        let payload = match &result {
            Ok(code) => LaunchExitPayload {
                error: None,
                exit_code: *code,
            },
            Err(LauncherError::Cancelled) => LaunchExitPayload {
                error: None,
                exit_code: None,
            },
            Err(e) => LaunchExitPayload {
                error: Some(e.to_string()),
                exit_code: None,
            },
        };
        let _ = app.emit(events::LAUNCH_EXIT, payload);
    });
}

/// 结束游戏进程树（taskkill /T 保证 LWJGL 子进程一并退出）
///
/// 该函数由同步 command 调用（跑在主线程），因此 `taskkill` 的进程创建
/// 必须丢到阻塞线程池，不能在这里 `spawn()` 同步等。
pub fn cancel(app: &AppHandle) {
    if let Some(token) = app
        .state::<AppState>()
        .cancels
        .lock()
        .unwrap()
        .get("launch")
    {
        token.cancel();
    }
    let pid = app.state::<AppState>().take_launch_pid();
    if let Some(pid) = pid {
        tauri::async_runtime::spawn_blocking(move || {
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .output();
        });
    }
}

pub fn status(app: &AppHandle) -> LaunchStatus {
    // 只取一次锁：过去这里在结构体字面量里连写两次 lock()，
    // 而第一个 MutexGuard 会活到整条语句结束 → 同线程重入 → 永久死锁。
    let pid = app.state::<AppState>().launch_pid();
    LaunchStatus {
        running: pid.is_some(),
        pid,
    }
}

async fn run(
    app: &AppHandle,
    options: LaunchOptions,
    token: &CancellationToken,
) -> Result<Option<i32>> {
    emit_log(app, "① 检查 Java 8 运行时…");
    let java_bin = java::ensure_installed(app, token).await?;

    emit_log(app, "② 检查 Forge 1.8.9…");
    forge::ensure_installed(app, token).await?;

    // Forge 只会加载 mods/ 下的 jar，客户端本体必须是其中一个 mod
    emit_log(app, "③ 检查客户端本体…");
    crate::client::ensure_installed(app, token).await?;

    emit_log(app, "④ 加载版本信息…");
    let mut profile = mojang::load_merged_profile(token).await?;
    profile.id = forge::version_id();

    emit_log(app, "⑤ 下载 Minecraft 本体…");
    let client_jar = mojang::ensure_client_jar(app, &profile, token).await?;

    emit_log(app, "⑥ 下载依赖库…");
    let libs = mojang::ensure_libraries(app, &profile, token).await?;

    emit_log(app, "⑦ 检查游戏资源…");
    let assets_id = mojang::ensure_assets(app, &profile, token).await?;

    emit_log(app, "⑧ 组装启动命令…");
    let p = paths::get();
    let settings = app.state::<AppState>().settings_snapshot();

    let mut classpath = String::new();
    classpath.push_str(&client_jar.to_string_lossy());
    for lib in &libs {
        classpath.push(';');
        classpath.push_str(&lib.to_string_lossy());
    }

    let natives_dir = p.natives_dir(&profile.id);
    let main_class = profile
        .main_class
        .clone()
        .unwrap_or_else(|| "net.minecraft.client.main.Main".into());

    let mut args: Vec<String> = vec![
        format!("-Xmx{}M", settings.ram_mb),
        // 默认堆底 512M：减少运行期堆扩容的卡顿；G1 在 1.8.9 上比默认
        // 的 Parallel GC 停顿更短。用户可用 extraJvmArgs 覆盖（JVM 取后者）。
        "-Xms512M".into(),
        "-XX:+UseG1GC".into(),
        "-Dfml.ignoreInvalidMinecraftCertificates=true".into(),
        "-Dfml.ignorePatchDiscrepancies=true".into(),
        format!("-Djava.library.path={}", natives_dir.display()),
    ];
    args.extend(settings.extra_jvm_args.iter().cloned());
    args.push("-cp".into());
    args.push(classpath);
    args.push(main_class);

    // V14 游戏参数模板替换
    let (player_name, auth_uuid, access_token, user_type) = match &options.account_id {
        Some(id) => {
            emit_log(app, "使用微软账号，检查令牌有效性…");
            let acc = crate::auth::get(id)?;
            let acc = crate::auth::ensure_fresh(&acc).await?;
            emit_log(app, &format!("已登录: {}", acc.name));
            (
                acc.name,
                Some(acc.id),
                Some(acc.mc_access_token),
                Some("msa".to_string()),
            )
        }
        None => {
            let name = options.player_name.trim().to_string();
            if name.is_empty() {
                return Err(LauncherError::Message("离线模式需要玩家名".into()));
            }
            (name, None, None, None)
        }
    };
    let offline_uuid = offline_uuid(&player_name);
    let template = profile
        .minecraft_arguments
        .clone()
        .ok_or_else(|| LauncherError::Message("版本 JSON 缺少 minecraftArguments".into()))?;
    let assets_root = p.assets_dir();

    let vars: Vec<(&str, String)> = vec![
        ("auth_player_name", player_name.clone()),
        ("version_name", profile.id.clone()),
        ("game_directory", p.instance_dir.display().to_string()),
        ("assets_root", assets_root.display().to_string()),
        ("assets_index_name", assets_id),
        (
            "auth_uuid",
            auth_uuid.unwrap_or_else(|| offline_uuid.clone()),
        ),
        (
            "auth_access_token",
            access_token.unwrap_or_else(|| "0".into()),
        ),
        ("user_properties", "{}".into()),
        ("user_type", user_type.unwrap_or_else(|| "legacy".into())),
        (
            "version_type",
            profile.version_type.clone().unwrap_or_default(),
        ),
        ("launcher_name", "AirLauncher".into()),
        ("launcher_version", env!("CARGO_PKG_VERSION").into()),
    ];
    for part in template.split(' ') {
        if let Some(key) = part.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
            match vars.iter().find(|(k, _)| *k == key) {
                Some((_, v)) => args.push(v.clone()),
                None => return Err(LauncherError::Message(format!("未知的参数模板: {key}"))),
            }
        } else {
            args.push(part.to_string());
        }
    }

    emit_log(app, "⑨ 启动游戏进程…");
    let mut command = tokio::process::Command::new(&java_bin);
    command
        .args(&args)
        .current_dir(&p.instance_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // 隐藏 java 控制台窗口，游戏窗口不受影响

    let mut child = command
        .spawn()
        .map_err(|e| LauncherError::Message(format!("游戏进程启动失败: {e}")))?;
    let pid = child.id();
    app.state::<AppState>().set_launch_pid(pid);
    emit_log(app, &format!("游戏已启动 (PID {})", pid.unwrap_or(0)));

    // 启动前把上次的 latest.txt 轮转归档，游戏日志落到 <instance>/logs/latest.txt
    let logs_dir = p.logs_dir.clone();
    rotate_game_logs(app, &logs_dir);

    // stdout/stderr 转发（合批发 UI，同时逐行落盘）
    if let Some(stdout) = child.stdout.take() {
        spawn_log_forward(app.clone(), stdout, logs_dir.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_log_forward(app.clone(), stderr, logs_dir);
    }

    // 等待退出或取消；退出码要透传给前端——正常退出与崩溃在 UI 上必须可区分
    let wait = async { child.wait().await };
    let code: Option<i32> = if let Some(joined) = token.run_until_cancelled(wait).await {
        joined.ok().and_then(|s| s.code())
    } else {
        // 用户在游戏运行期间点了取消
        if let Some(pid) = pid {
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .creation_flags(0x0800_0000)
                .spawn();
        }
        return Err(LauncherError::Cancelled);
    };

    app.state::<AppState>().set_launch_pid(None);
    match code {
        Some(0) => emit_log(app, "游戏进程已正常退出"),
        Some(c) => {
            emit_log(
                app,
                &format!(
                    "游戏进程异常退出（exit code {c}），可在日志目录的 latest.txt 中查找崩溃原因"
                ),
            );
        }
        None => emit_log(app, "游戏进程已退出（被外部终止）"),
    }
    Ok(Some(code.unwrap_or(0)))
}

/// 日志轮转：latest.txt -> latest-N.txt，保留最近 KEEP_LOGS 份
const KEEP_LOGS: u32 = 5;

fn rotate_game_logs(app: &AppHandle, logs_dir: &std::path::Path) {
    let latest = logs_dir.join("latest.txt");
    if !latest.exists() {
        return;
    }
    // 找已有归档的最大序号
    let mut max = 0u32;
    if let Ok(entries) = std::fs::read_dir(logs_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(num) = name
                .strip_prefix("latest-")
                .and_then(|s| s.strip_suffix(".txt"))
                .and_then(|s| s.parse::<u32>().ok())
            {
                max = max.max(num);
            }
        }
    }
    // 删最旧的（超出保留数的整组清掉）
    for n in max.saturating_sub(KEEP_LOGS - 1) + 1..=max {
        let _ = std::fs::remove_file(logs_dir.join(format!("latest-{n}.txt")));
    }
    let _ = std::fs::rename(&latest, logs_dir.join(format!("latest-{}.txt", max + 1)));
    emit_log(app, &format!("日志目录: {}", logs_dir.display()));
}

/// 转发游戏 stdout/stderr 到 UI 与 <instance>/logs/latest.txt。
///
/// 必须合批：Minecraft 启动期每秒可输出数百行，逐行 `emit` 会让主线程
/// 频繁排队处理 IPC，明显拖慢窗口响应。这里按 100ms 或 40 行先攒后发，
/// 一次 emit 多行（前端按行拆分）。
///
/// 文件落盘每行一次小 append 写，量级完全可接受（std::fs append 是行缓冲外的
/// 同步写，但 MC 每秒几百行 × ~80 字节对磁盘毫无压力），换来崩溃后总有日志可查。
fn spawn_log_forward(
    app: AppHandle,
    stream: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    logs_dir: std::path::PathBuf,
) {
    tauri::async_runtime::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::time::{Duration, Instant};

        const FLUSH_INTERVAL: Duration = Duration::from_millis(100);
        const FLUSH_LINES: usize = 40;

        // 两个流（stdout/stderr）各自持有一个 append 句柄；目录已由 rotate 前置创建
        let _ = std::fs::create_dir_all(&logs_dir);
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(logs_dir.join("latest.txt"))
            .ok();
        let log_file = std::sync::Mutex::new(log_file);
        let write_line = |line: &str| {
            if let Ok(mut f) = log_file.lock()
                && let Some(f) = f.as_mut()
            {
                use std::io::Write;
                let _ = writeln!(f, "{line}");
            }
        };

        let reader = BufReader::new(stream);
        let mut lines = reader.lines();
        let mut batch: Vec<String> = Vec::with_capacity(FLUSH_LINES);
        let mut last_flush = Instant::now();

        let flush = |app: &AppHandle, batch: &mut Vec<String>| {
            if batch.is_empty() {
                return;
            }
            let payload = batch.join("\n");
            batch.clear();
            let _ = app.emit(events::LAUNCH_LOG, payload);
        };

        loop {
            let next = tokio::time::timeout(FLUSH_INTERVAL, lines.next_line()).await;
            match next {
                Ok(Ok(Some(line))) => {
                    batch.push(format!("[mc] {line}"));
                    write_line(&line);
                    if batch.len() >= FLUSH_LINES || last_flush.elapsed() >= FLUSH_INTERVAL {
                        flush(&app, &mut batch);
                        last_flush = Instant::now();
                    }
                }
                Ok(Ok(None)) => break,
                Ok(Err(_)) => break,
                // 超时：流暂时没新行，把已攒的发出去
                Err(_) => {
                    flush(&app, &mut batch);
                    last_flush = Instant::now();
                }
            }
        }
        flush(&app, &mut batch);
    });
}

/// 离线模式 UUID：MD5(name 空补) 的 UUID v3，与官方离线规则一致
fn offline_uuid(name: &str) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(format!("OfflinePlayer:{name}"));
    // Builder::from_md5_bytes 自动设置 v3 的 version/variant 位
    let hash: [u8; 16] = hasher.finalize().into();
    uuid::Builder::from_md5_bytes(hash).into_uuid().to_string()
}

fn emit_log(app: &AppHandle, msg: &str) {
    let _ = app.emit(events::LAUNCH_LOG, msg.to_string());
}
