/*
 * AirLauncher
 * A Tauri-based launcher for the AirPlus Minecraft client.
 * https://github.com/lmx0721/AirPlus
 *
 * Java 8 运行时，按优先级解析：
 *   0) 用户手动指定的 Java（settings.javaPath，设置页「浏览…」）
 *   1) 内置 JRE（<数据目录>/jre）
 *   2) 系统已安装的 Java 8（JAVA_HOME / PATH / 注册表 / 常见安装目录）
 *      —— 结果持久化到 settings.json，冷启动先校验缓存路径仍存在再决定是否扫描
 *   3) 自动下载 Adoptium Temurin 8 JRE（优先清华 TUNA 镜像，回退官方 CDN，
 *      下载后用 API 返回的 sha256 校验）
 * Forge 1.8.9 安装与游戏运行均使用解析出的 Java 8。
 */
use crate::download;
use crate::error::{LauncherError, Result};
use crate::events;
use crate::paths;
use crate::state::AppState;
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::RwLock;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

const ADOPTIUM_API: &str = "https://api.adoptium.net/v3/assets/latest/8/hotspot?architecture=x64&image_type=jre&os=windows";
/// 清华 TUNA 镜像 Adoptium 归档（与 API 返回的 package.name 同名文件）
const ADOPTIUM_TUNA: &str = "https://mirrors.tuna.tsinghua.edu.cn/Adoptium/8/jre/x64/windows/";

#[derive(Deserialize)]
struct AdoptiumAsset {
    binary: AdoptiumBinary,
}

#[derive(Deserialize)]
struct AdoptiumBinary {
    package: AdoptiumPackage,
}

#[derive(Deserialize)]
struct AdoptiumPackage {
    link: String,
    name: String,
    /// Adoptium v3 API 返回的 sha256（小写 hex），用于校验 40MB 的 zip
    #[serde(default)]
    checksum: Option<String>,
}

fn java_exe() -> std::path::PathBuf {
    paths::get().jre_java_exe()
}

pub fn is_installed() -> bool {
    java_exe().exists()
}

// ---------- 版本信息 ----------

/// 一个可用的 Java：路径 + `java -version` 解析出的完整版本号与发行商
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaInfo {
    pub path: String,
    /// 完整版本号，如 "1.8.0_402" / "17.0.10"
    pub version: String,
    /// 发行商简称，如 "Temurin" / "Zulu" / "GraalVM"（解析不出来为 None）
    pub vendor: Option<String>,
}

impl JavaInfo {
    /// 设置页 / 主页展示用的一行描述：「Java 1.8.0_402 · Temurin」
    pub fn describe(&self) -> String {
        match &self.vendor {
            Some(v) => format!("Java {} · {}", self.version, v),
            None => format!("Java {}", self.version),
        }
    }
}

/// 手动检测结果：Java 8 与其它版本分开报告，
/// 让「装了 Java 17 但没有 Java 8」的用户明白启动器会自动下载，而不是误以为出了问题。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DetectResult {
    pub java8: Option<JavaInfo>,
    pub others: Vec<JavaInfo>,
}

/// 当前生效的 Java（主页/设置页展示用）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaStatus {
    /// "manual" | "bundled" | "system" | "download" | "none"
    pub source: String,
    pub path: Option<String>,
    pub version: Option<String>,
    pub vendor: Option<String>,
}

// ---------- 系统 Java 扫描 ----------

/// 本次会话内缓存的系统 Java，避免每次启动游戏都重新扫描
static SYSTEM_JAVA: RwLock<Option<JavaInfo>> = RwLock::new(None);

fn system_cache_get() -> Option<JavaInfo> {
    SYSTEM_JAVA
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn system_cache_set(info: Option<JavaInfo>) {
    *SYSTEM_JAVA.write().unwrap_or_else(|e| e.into_inner()) = info;
}

/// 只接受 Java 8：Forge 1.8.9 / MC 1.8.9 在 Java 9+ 上会因模块系统等问题无法运行
const REQUIRED_JAVA_MAJOR: u32 = 8;

/// 优先读缓存；缓存为空或路径已失效（用户卸载了 Java）时重新扫描
async fn find_system_java_cached() -> Option<JavaInfo> {
    if let Some(info) = system_cache_get()
        && std::path::Path::new(&info.path).exists()
    {
        return Some(info);
    }
    let found = scan_system_javas().await.java8;
    system_cache_set(found.clone());
    found
}

/// 手动触发一次全量扫描（设置页「检测系统 Java」用），结果同样写回缓存
pub async fn detect_system_java_fresh() -> DetectResult {
    let result = scan_system_javas().await;
    system_cache_set(result.java8.clone());
    result
}

/// 在本机查找所有可用的 Java。
///
/// 候选按可信度依次收集：`JAVA_HOME`、`PATH`、注册表 JavaSoft/JDK 项、
/// 常见安装目录。候选并发跑 `java -version` 验证（每次几十毫秒），
/// 收集**所有**版本：Java 8 直接可用；其它版本（11/17/21…）对 1.8.9 无用，
/// 但要报告给用户（「检测到 Java 17，将自动下载 Java 8」），避免误以为检测失败。
async fn scan_system_javas() -> DetectResult {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    collect_candidates(&mut candidates);

    // 去重
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|p| p.is_file() && seen.insert(p.to_string_lossy().to_lowercase()));

    // 并发验证：每个候选一次 `java -version`，最坏情况压到单次耗时
    let jobs = candidates
        .into_iter()
        .map(|path| tauri::async_runtime::spawn_blocking(move || java_info_of(&path)));
    let results = join_all(jobs).await;

    let mut java8: Option<JavaInfo> = None;
    let mut others: Vec<JavaInfo> = Vec::new();
    for (info, major) in results.into_iter().flatten().flatten() {
        if major == REQUIRED_JAVA_MAJOR && java8.is_none() {
            java8 = Some(info);
        } else {
            others.push(info);
        }
    }
    others.sort_by(|a, b| a.version.cmp(&b.version));
    DetectResult { java8, others }
}

#[cfg(windows)]
fn collect_candidates(out: &mut Vec<std::path::PathBuf>) {
    if let Ok(home) = std::env::var("JAVA_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            out.push(std::path::Path::new(home).join("bin").join("java.exe"));
        }
    }

    if let Ok(path_var) = std::env::var("PATH") {
        out.extend(std::env::split_paths(&path_var).map(|dir| dir.join("java.exe")));
    }

    registry_java_exes(out);
    common_dir_java_exes(out);
}

#[cfg(not(windows))]
fn collect_candidates(_out: &mut Vec<std::path::PathBuf>) {}

/// 注册表 JavaSoft 下的 JavaHome（经典 Oracle JRE 8 与较新 JDK 的注册位置）
#[cfg(windows)]
fn registry_java_exes(out: &mut Vec<std::path::PathBuf>) {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    const KEYS: &[&str] = &[
        r"SOFTWARE\JavaSoft\Java Runtime Environment",
        r"SOFTWARE\JavaSoft\Java Development Kit",
        r"SOFTWARE\JavaSoft\JDK",
        r"SOFTWARE\WOW6432Node\JavaSoft\Java Runtime Environment",
        r"SOFTWARE\WOW6432Node\JavaSoft\Java Development Kit",
    ];

    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for key_path in KEYS {
            let Ok(root) = RegKey::predef(hive).open_subkey(key_path) else {
                continue;
            };
            // 经典布局：CurrentVersion 指向具体版本子键
            if let Ok(current) = root.get_value::<String, _>("CurrentVersion")
                && let Ok(sub) = root.open_subkey(&current)
                && let Ok(home) = sub.get_value::<String, _>("JavaHome")
            {
                out.push(std::path::Path::new(&home).join("bin").join("java.exe"));
            }
            // 新布局：每个版本一个子键，各带 JavaHome
            for name in root.enum_keys().flatten() {
                if let Ok(sub) = root.open_subkey(&name)
                    && let Ok(home) = sub.get_value::<String, _>("JavaHome")
                {
                    out.push(std::path::Path::new(&home).join("bin").join("java.exe"));
                }
            }
        }
    }
}

/// 常见安装根目录下扫一层：`<root>\<发行商子目录>\*\bin\java.exe`
#[cfg(windows)]
fn common_dir_java_exes(out: &mut Vec<std::path::PathBuf>) {
    const SUBDIRS: &[&str] = &[
        "Java",
        "Eclipse Adoptium",
        "AdoptOpenJDK",
        "Zulu",
        "Amazon Corretto",
        "BellSoft",
        "Microsoft",
        "Semeru",
        "ojdkbuild",
    ];

    let roots: Vec<std::path::PathBuf> = [
        "ProgramW6432",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "LOCALAPPDATA",
    ]
    .iter()
    .filter_map(|var| std::env::var(var).ok())
    .map(std::path::PathBuf::from)
    .collect();

    for root in roots {
        for sub in SUBDIRS {
            let dir = root.join(sub);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let java = entry.path().join("bin").join("java.exe");
                out.push(java);
            }
        }
    }
}

/// 解析出的 `java -version` 信息：完整版本号 + 主版本号
#[derive(Debug, Clone, PartialEq)]
struct ParsedVersion {
    /// 引号里的原始版本串，如 "1.8.0_402" / "17.0.10"
    full: String,
    major: u32,
}

/// 从 `java -version` 输出解析版本串：
/// 老格式 `java version "1.8.0_401"` → major 8；新格式 `openjdk version "17.0.10"` → major 17
fn parse_version_string(version_output: &str) -> Option<ParsedVersion> {
    let start = version_output.find('"')? + 1;
    let rest = &version_output[start..];
    let end = rest.find('"')?;
    let version = &rest[..end];
    let mut parts = version.split('.');
    let first = parts.next()?;
    let major = if first == "1" {
        parts.next()?.parse().ok()?
    } else {
        first.parse().ok()?
    };
    Some(ParsedVersion {
        full: version.to_string(),
        major,
    })
}

/// 从输出里 "Runtime Environment" 后面的括号段解析发行商：
///   `OpenJDK Runtime Environment (Temurin-8u402-b06) (build ...)` → "Temurin"
///   `OpenJDK Runtime Environment (Zulu 8.78.0.19-CA-win64) ...` → "Zulu"
/// Oracle 的 `Java(TM) SE Runtime Environment (build 1.8.0_401-b10)`
/// 没有发行商名 → None。
/// 中文 Windows 上 stderr 可能是 GBK，from_utf8_lossy 会产生替换符，
/// 但版本行与括号内容都是 ASCII，不受影响。
fn parse_vendor(version_output: &str) -> Option<String> {
    let idx = version_output.find("Runtime Environment")?;
    let line = &version_output[idx..];
    let open = line.find('(')?;
    let close = line[open + 1..].find(')')? + open + 1;
    let token = &line[open + 1..close];
    // 排除 `(build ...)` 形式
    if token.starts_with("build ") {
        return None;
    }
    // 取第一段：Temurin-8u402-b06 → Temurin；Zulu 8.78 → Zulu
    let short: String = token
        .split(['-', ' '])
        .next()
        .unwrap_or_default()
        .to_string();
    if short.is_empty() { None } else { Some(short) }
}

/// 运行 `java -version` 并解析出完整信息（该命令把版本信息输出到 stderr）
fn java_info_of(java_exe: &std::path::Path) -> Option<(JavaInfo, u32)> {
    let output = run_java_version(java_exe)?;
    let parsed = parse_version_string(&output)?;
    let info = JavaInfo {
        path: java_exe.to_string_lossy().to_string(),
        version: parsed.full,
        vendor: parse_vendor(&output),
    };
    Some((info, parsed.major))
}

#[cfg(windows)]
fn run_java_version(java_exe: &std::path::Path) -> Option<String> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new(java_exe)
        .arg("-version")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stderr).into_owned())
}

#[cfg(not(windows))]
fn run_java_version(_java_exe: &std::path::Path) -> Option<String> {
    None
}

// ---------- 解析与持久化 ----------

fn emit_stage(app: &AppHandle, stage: &str) {
    if crate::warmup::active() {
        return; // 预热不打扰主界面日志
    }
    let _ = app.emit(events::LAUNCH_LOG, format!("[java] {stage}"));
}

/// 把解析结果写回 settings.json（javaSource + systemJavaPath），
/// 下次冷启动先校验缓存路径仍存在，避免重复读注册表 + 多次 `java -version`。
/// 只有 system 来源才写路径；其它来源不动 systemJavaPath，避免抹掉缓存。
fn persist_choice(app: &AppHandle, source: &str, system_path: Option<&str>) {
    let state = app.state::<AppState>();
    let snapshot = {
        let mut s = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        s.java_source = Some(source.to_string());
        if let Some(p) = system_path {
            s.system_java_path = Some(p.to_string());
        }
        s.clone()
    };
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = snapshot.save(&paths::get().settings_file) {
            eprintln!("持久化 Java 检测结果失败: {e}");
        }
    });
}

/// 解析最终使用的 Java。`allow_download` 为 false 时（预热/状态查询）只做本地解析，
/// 找不到就返回 None，不触发联网下载；`log = false` 时不向主界面日志播报。
///
/// 优先级：手动指定 > 内置 JRE > 系统 Java 8（设置缓存 → 现场扫描）> 自动下载。
async fn resolve(
    app: &AppHandle,
    allow_download: bool,
    token: &CancellationToken,
    log: bool,
) -> Result<Option<PathBuf>> {
    let settings = app.state::<AppState>().settings_snapshot();

    // 0) 手动指定：最高优先级；路径失效时提示并继续往后找
    if let Some(manual) = settings.java_path.clone() {
        let p = std::path::PathBuf::from(&manual);
        if p.exists() {
            if log {
                emit_stage(app, &format!("使用手动指定的 Java: {manual}"));
            }
            persist_choice(app, "manual", None);
            return Ok(Some(p));
        }
        if log {
            emit_stage(app, &format!("手动指定的 Java 路径不存在: {manual}"));
        }
    }

    // 1) 内置 JRE
    let bundled = java_exe();
    if is_installed() {
        persist_choice(app, "bundled", None);
        return Ok(Some(bundled));
    }

    // 2) 系统 Java 8：先看上次持久化的路径（冷启动免扫描），失效再现场扫描
    if settings.java_source.as_deref() == Some("system")
        && let Some(p) = settings.system_java_path.clone()
        && std::path::Path::new(&p).exists()
    {
        if log {
            emit_stage(app, &format!("使用系统 Java: {p}"));
        }
        return Ok(Some(std::path::PathBuf::from(p)));
    }
    if let Some(info) = find_system_java_cached().await {
        if log {
            emit_stage(
                app,
                &format!("使用系统 Java: {} ({})", info.path, info.describe()),
            );
        }
        persist_choice(app, "system", Some(&info.path));
        return Ok(Some(std::path::PathBuf::from(info.path)));
    }

    // 3) 自动下载（预热模式不下载；只可能是正式启动，日志保持播报）
    if !allow_download {
        return Ok(None);
    }
    let path = download_temurin(app, token).await?;
    persist_choice(app, "download", None);
    Ok(Some(path))
}

/// 确保可用的 Java 8（幂等，可取消），返回最终使用的 java.exe 路径。
pub async fn ensure_installed(app: &AppHandle, token: &CancellationToken) -> Result<PathBuf> {
    resolve(app, true, token, true)
        .await?
        .ok_or_else(|| LauncherError::Message("Java 8 运行时不可用".into()))
}

/// 预热：只做本地解析（内置 JRE / 系统 Java 扫描并写缓存），不联网下载、不播报
pub async fn warm_resolve(app: &AppHandle) {
    let token = CancellationToken::new();
    let _ = resolve(app, false, &token, false).await;
}

/// 当前生效的 Java（主页/设置页展示）。只做本地解析，不触发下载、不播报。
pub async fn status(app: &AppHandle) -> JavaStatus {
    match resolve(app, false, &CancellationToken::new(), false).await {
        Ok(Some(path)) => {
            // 手动/系统路径能顺带给出版本信息更好，失败就只报路径
            let probe = path.clone();
            let info = tauri::async_runtime::spawn_blocking(move || java_info_of(&probe))
                .await
                .ok()
                .flatten()
                .map(|(info, _)| info);
            let source = app
                .state::<AppState>()
                .settings_snapshot()
                .java_source
                .unwrap_or_else(|| "bundled".into());
            match info {
                Some(info) => JavaStatus {
                    source,
                    version: Some(info.version),
                    vendor: info.vendor,
                    path: Some(info.path),
                },
                None => JavaStatus {
                    source,
                    path: Some(path.to_string_lossy().to_string()),
                    version: None,
                    vendor: None,
                },
            }
        }
        Ok(None) => JavaStatus {
            source: "none".into(),
            path: None,
            version: None,
            vendor: None,
        },
        Err(_) => JavaStatus {
            source: "none".into(),
            path: None,
            version: None,
            vendor: None,
        },
    }
}

/// 查询 Adoptium 拿到 (下载链接们, 文件名, sha256)
async fn fetch_adoptium_asset(
    token: &CancellationToken,
) -> Result<(Vec<String>, String, Option<String>)> {
    let fut = crate::net::get().get(ADOPTIUM_API).send();
    let resp = token
        .run_until_cancelled(fut)
        .await
        .ok_or(LauncherError::Cancelled)?
        .map_err(|e| LauncherError::Message(format!("Adoptium API 请求失败: {e}")))?
        .error_for_status()
        .map_err(|e| LauncherError::Message(format!("Adoptium API 返回错误: {e}")))?;
    let assets: Vec<AdoptiumAsset> = resp
        .json()
        .await
        .map_err(|e| LauncherError::Message(format!("Adoptium API 解析失败: {e}")))?;
    let asset = assets
        .first()
        .ok_or_else(|| LauncherError::Message("Adoptium 未返回可用 JRE 8".into()))?;

    // 首选 TUNA 镜像（国内快），回退官方 CDN
    let mut urls = vec![format!("{}{}", ADOPTIUM_TUNA, asset.binary.package.name)];
    urls.push(asset.binary.package.link.clone());
    Ok((
        urls,
        asset.binary.package.name.clone(),
        asset.binary.package.checksum.clone(),
    ))
}

/// 下载 Temurin 8 JRE 并解压到内置 JRE 目录。
/// 有 sha256 时下载后校验（TUNA 与官方 CDN 的同一文件 sha256 相同）。
async fn download_temurin(app: &AppHandle, token: &CancellationToken) -> Result<PathBuf> {
    emit_stage(app, "未找到系统 Java 8，准备自动下载…");
    emit_stage(app, "查询 Adoptium Temurin 8 JRE…");
    let (urls, file_name, sha256) = fetch_adoptium_asset(token).await?;

    let zip_path = download::cache_path(&file_name);
    // 校验通过过的 zip 直接复用；从未校验过的缓存（旧版本产物）重新下载
    let marker = zip_path.with_extension("zip.ok");
    if !zip_path.exists() || !marker.exists() {
        emit_stage(app, "下载 Temurin 8 JRE…");
        let mut last_err: Option<LauncherError> = None;
        let mut ok = false;
        for url in &urls {
            match download::fetch_to(
                app,
                "jre-download",
                "Temurin 8 JRE",
                url,
                &zip_path,
                None,
                token,
            )
            .await
            {
                Ok(_) => {
                    ok = true;
                    break;
                }
                Err(LauncherError::Cancelled) => return Err(LauncherError::Cancelled),
                Err(e) => {
                    last_err = Some(e);
                    emit_stage(app, "下载源失败，尝试下一个…");
                }
            }
        }
        if !ok {
            return Err(last_err
                .unwrap_or_else(|| LauncherError::Message("JRE 下载失败（未知原因）".into())));
        }

        // sha256 校验：Adoptium 官方 API 提供期望值，40MB 的 zip 不校验属于遗漏
        if let Some(expected) = &sha256 {
            emit_stage(app, "校验 JRE 压缩包…");
            let zip = zip_path.clone();
            let expected = expected.to_lowercase();
            let valid = tauri::async_runtime::spawn_blocking(move || {
                crate::download::sha256_file_sync(&zip).ok() == Some(expected)
            })
            .await
            .unwrap_or(false);
            if !valid {
                let _ = std::fs::remove_file(&zip_path);
                return Err(LauncherError::Checksum(
                    "Temurin 8 JRE zip 的 sha256 校验失败，已删除损坏的缓存".into(),
                ));
            }
        }
        std::fs::write(&marker, b"ok")?;
    }

    emit_stage(app, "解压 JRE…");
    // 解压时去掉 zip 内层目录（如 jdk8u512-b09-jre/），使 java.exe 落在 jre/bin/java.exe
    let zip_c = zip_path.clone();
    let dest = paths::get().jre_dir.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<()> { extract_jre(&zip_c, &dest) })
        .await
        .map_err(|e| LauncherError::Message(format!("JRE 解压任务失败: {e}")))??;

    if !is_installed() {
        return Err(LauncherError::Message(
            "JRE 解压完成但未找到 bin/java.exe".into(),
        ));
    }
    emit_stage(app, "JRE 就绪");
    Ok(java_exe())
}

/// 同步解压：如果 zip 只有一个内层根目录，把它拍平
fn extract_jre(zip_path: &std::path::Path, dest: &std::path::Path) -> Result<()> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| LauncherError::Message(format!("JRE zip 读取失败: {e}")))?;

    // 判断内层目录
    let mut common_root: Option<String> = None;
    for i in 0..archive.len() {
        let name = archive
            .by_index_raw(i)
            .map_err(|e| LauncherError::Message(format!("JRE zip 条目读取失败 (#{i}): {e}")))?
            .name()
            .to_string();
        let Some(first) = name.split('/').next() else {
            continue;
        };
        if first.is_empty() || name == first {
            common_root = None;
            break;
        }
        match &common_root {
            None => common_root = Some(first.to_string()),
            Some(r) if r != first => {
                common_root = None;
                break;
            }
            _ => {}
        }
    }
    let root = common_root.unwrap_or_default();
    let root_prefix = if root.is_empty() {
        String::new()
    } else {
        format!("{root}/")
    };

    std::fs::create_dir_all(dest)?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| LauncherError::Message(format!("JRE zip 条目读取失败 (#{i}): {e}")))?;
        let name = entry.name().to_string();
        let Some(rel) = name.strip_prefix(&root_prefix) else {
            continue;
        };
        if rel.is_empty() || name.ends_with('/') {
            continue;
        }
        let dest_path = dest.join(rel);
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&dest_path)?;
        std::io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_java_version_variants() {
        let out = "openjdk version \"1.8.0_402\" \nOpenJDK Runtime Environment (Temurin-8u402-b06) (build 1.8.0_402-b06)\nOpenJDK 64-Bit Server VM (Temurin-8u402-b06) (build 25.402-b06, mixed mode)";
        let parsed = parse_version_string(out).unwrap();
        assert_eq!(parsed.major, 8);
        assert_eq!(parsed.full, "1.8.0_402");
        assert_eq!(parse_vendor(out).as_deref(), Some("Temurin"));

        let parsed = parse_version_string("java version \"1.8.0_401\"").unwrap();
        assert_eq!(parsed.major, 8);
        assert_eq!(parsed.full, "1.8.0_401");

        let parsed = parse_version_string("openjdk version \"17.0.10\"").unwrap();
        assert_eq!(parsed.major, 17);
        assert_eq!(parsed.full, "17.0.10");

        let parsed = parse_version_string("openjdk version \"21.0.2\"").unwrap();
        assert_eq!(parsed.major, 21);

        assert!(parse_version_string("no version here").is_none());
        assert!(parse_version_string("bad \"x.y\"").is_none());
    }

    #[test]
    fn parse_vendor_variants() {
        let zulu = "openjdk version \"1.8.0_412\"\nOpenJDK Runtime Environment (Zulu 8.78.0.19-CA-win64) (build 1.8.0_412-b08)";
        assert_eq!(parse_vendor(zulu).as_deref(), Some("Zulu"));

        // Oracle 经典输出：括号里是 build，解析不出发行商
        let oracle =
            "java version \"1.8.0_401\"\nJava(TM) SE Runtime Environment (build 1.8.0_401-b10)";
        assert_eq!(parse_vendor(oracle), None);

        let graal = "openjdk version \"17.0.3\"\nOpenJDK Runtime Environment (GraalVM CE 22.1.0) (build 17.0.3+7-jvmci-22.1-b06)";
        assert_eq!(parse_vendor(graal).as_deref(), Some("GraalVM"));

        // GBK 乱码不影响 ASCII 的版本行解析
        let gbk = "openjdk version \"1.8.0_402\"\nOpenJDK Runtime Environment (\u{fffd}\u{fffd}) (build 1.8.0_402-b06)";
        assert_eq!(parse_version_string(gbk).map(|p| p.major), Some(8));
    }

    /// AIRLAUNCHER_TEST_JRE_ZIP 指向 Temurin 8 JRE zip 时运行
    #[test]
    fn extract_real_jre() {
        let Ok(path) = std::env::var("AIRLAUNCHER_TEST_JRE_ZIP") else {
            eprintln!("跳过：未设置 AIRLAUNCHER_TEST_JRE_ZIP");
            return;
        };
        let tmp = std::env::temp_dir().join(format!("airlauncher-jre-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);

        extract_jre(std::path::Path::new(&path), &tmp).unwrap();

        assert!(tmp.join("bin/java.exe").exists(), "bin/java.exe 未拍平解出");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
