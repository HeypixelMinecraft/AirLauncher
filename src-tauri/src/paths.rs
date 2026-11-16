/*
 * AirLauncher
 * A Tauri-based launcher for the AirPlus Minecraft client.
 * https://github.com/lmx0721/AirPlus
 *
 * 所有路径均在运行时从数据根目录派生，禁止硬编码绝对路径。
 *
 * 数据目录默认是 `%LOCALAPPDATA%/<identifier>`，也可以在设置里改到任意位置。
 * 无论默认还是自定义，启动器都会在数据目录下自动创建 `AirLauncher` 文件夹，
 * 把全部数据（实例、游戏、JRE、缓存、配置）收纳在里面；
 * 用户手动把数据目录指到某个名为 AirLauncher 的文件夹时不再重复嵌套。
 *
 * 指向自定义目录的信息必须放在一个「位置固定」的引导文件里（默认目录下的
 * launcher.json），否则搬走数据后启动器就再也找不到自己的数据了。
 */
use crate::error::{LauncherError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tauri::Manager;

static PATHS: OnceLock<AppPaths> = OnceLock::new();

/// 引导文件：只记录「数据目录在哪」，始终留在默认目录下
const BOOTSTRAP_FILE: &str = "launcher.json";

/// 数据目录下的数据子文件夹：所有数据、游戏等全部收纳在里面
const DATA_FOLDER: &str = "AirLauncher";

/// 迁移数据目录时要整体搬走的内容
const MIGRATE_ENTRIES: &[&str] = &[
    "instances",
    "jre",
    "cache",
    "settings.json",
    "accounts.json",
    "hang.log",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Bootstrap {
    data_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPaths {
    /// 数据根目录（= 数据目录下的 AirLauncher 子文件夹，可自定义数据目录）
    pub root: PathBuf,
    /// 默认数据目录（引导文件所在位置，「恢复默认」用）
    pub default_root: PathBuf,
    /// 实例总目录
    pub instances_dir: PathBuf,
    /// 唯一实例根（游戏工作目录）
    pub instance_dir: PathBuf,
    /// 实例 mods 目录
    pub mods_dir: PathBuf,
    /// JRE 安装目录
    pub jre_dir: PathBuf,
    /// 游戏日志目录（<instance>/logs，latest.txt 与轮转归档）
    pub logs_dir: PathBuf,
    /// 下载缓存目录（installer、临时文件）
    pub cache_dir: PathBuf,
    /// settings.json
    pub settings_file: PathBuf,
    /// accounts.json
    pub accounts_file: PathBuf,
    /// launcher-manifest.json（下载清单：客户端版本 + 模组 + sha1）
    pub manifest_file: PathBuf,
}

impl AppPaths {
    fn build(app: &tauri::AppHandle) -> AppPaths {
        let default_root = app
            .path()
            .app_local_data_dir()
            .expect("无法确定应用数据目录");

        let mut base = default_root.clone();
        if let Some(custom) = read_bootstrap(&default_root) {
            let p = PathBuf::from(&custom);
            if p.is_absolute() {
                base = p;
            } else {
                eprintln!("自定义数据目录不是绝对路径，忽略：{custom}");
            }
        }

        let root = match prepare(&base) {
            Some(r) => r,
            None => {
                // 自定义目录可能被删了 / 移动硬盘没插：回退默认目录，别让启动器起不来
                eprintln!(
                    "自定义数据目录不可用，回退到默认目录 {}",
                    default_root.display()
                );
                base = default_root.clone();
                prepare(&base).expect("无法创建启动器目录结构")
            }
        };

        Self::from_root(root, default_root)
    }

    fn from_root(root: PathBuf, default_root: PathBuf) -> AppPaths {
        let instances_dir = root.join("instances");
        let instance_dir = instances_dir.join("airplus-b4");
        let mods_dir = instance_dir.join("mods");
        let jre_dir = root.join("jre");
        let cache_dir = root.join("cache");
        let logs_dir = instance_dir.join("logs");

        AppPaths {
            settings_file: root.join("settings.json"),
            accounts_file: root.join("accounts.json"),
            manifest_file: instance_dir.join(crate::manifest::FILE_NAME),
            root,
            default_root,
            instances_dir,
            instance_dir,
            mods_dir,
            jre_dir,
            logs_dir,
            cache_dir,
        }
    }

    pub fn jre_java_exe(&self) -> PathBuf {
        self.jre_dir.join("bin").join("java.exe")
    }

    /// 实例内的游戏子目录（versions/libraries/assets/natives）
    pub fn versions_dir(&self) -> PathBuf {
        self.instance_dir.join("versions")
    }

    pub fn libraries_dir(&self) -> PathBuf {
        self.instance_dir.join("libraries")
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.instance_dir.join("assets")
    }

    pub fn natives_dir(&self, version: &str) -> PathBuf {
        self.instance_dir.join("natives").join(version)
    }
}

fn ensure_dirs(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root.join("instances").join("airplus-b4").join("mods"))?;
    std::fs::create_dir_all(root.join("jre"))?;
    std::fs::create_dir_all(root.join("cache"))?;
    Ok(())
}

/// 数据目录里的实际数据根：自动创建 AirLauncher 子文件夹，所有数据、游戏等全部收在里面。
/// 用户手动把数据目录指到某个名为 AirLauncher 的文件夹时不再重复嵌套。
fn resolve_root(base: &Path) -> PathBuf {
    let self_named = base
        .file_name()
        .map(|n| n.eq_ignore_ascii_case(DATA_FOLDER))
        .unwrap_or(false);
    if self_named {
        base.to_path_buf()
    } else {
        base.join(DATA_FOLDER)
    }
}

/// 确保数据目录存在、自动创建 AirLauncher 数据子目录，
/// 并把旧版本直接散落在数据目录下的数据搬进去。
fn prepare(base: &Path) -> Option<PathBuf> {
    std::fs::create_dir_all(base).ok()?;
    let root = resolve_root(base);
    migrate_legacy(base, &root);
    ensure_dirs(&root).ok()?;
    Some(root)
}

/// 旧版本把数据直接放在数据目录下：升级后全部搬进 AirLauncher 子目录。
/// 新目录已存在同名内容时保留新目录，源文件原样留着不动。
fn migrate_legacy(base: &Path, root: &Path) {
    for name in MIGRATE_ENTRIES {
        let src = base.join(name);
        let dst = root.join(name);
        if !src.exists() || dst.exists() || src == dst {
            continue;
        }
        if let Err(e) = move_entry(&src, &dst) {
            eprintln!("迁移旧数据 {name} 失败（{e}）");
        }
    }
}

fn read_bootstrap(default_root: &Path) -> Option<String> {
    let bytes = std::fs::read(default_root.join(BOOTSTRAP_FILE)).ok()?;
    let parsed: Bootstrap = serde_json::from_slice(&bytes).ok()?;
    parsed.data_dir.filter(|s| !s.trim().is_empty())
}

/// 去掉 Windows 的 `\\?\` 前缀：canonicalize 的产物直接存盘/显示会很难看，
/// 而且用户手动输入路径时通常不带这个前缀，统一成普通形式便于比较。
fn clean_path(p: PathBuf) -> PathBuf {
    let text = p.to_string_lossy().to_string();
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        if let Some(unc) = rest.strip_prefix("UNC\\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        return PathBuf::from(rest);
    }
    p
}

fn write_bootstrap(default_root: &Path, data_dir: Option<&Path>) -> Result<()> {
    std::fs::create_dir_all(default_root)?;
    let file = default_root.join(BOOTSTRAP_FILE);
    let body = serde_json::to_vec_pretty(&Bootstrap {
        data_dir: data_dir.map(|p| p.to_string_lossy().to_string()),
    })?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &file)?;
    Ok(())
}

/// 切换数据目录：把现有数据整体搬到新目录（自动套上 AirLauncher 子文件夹），
/// 并写入引导文件。
///
/// 返回实际的数据根目录（新目录下的 AirLauncher 文件夹）；调用方（前端）需要
/// 重启启动器让新路径生效——`AppPaths` 是进程内只读的，运行中改不了。
pub fn set_data_dir(raw: &str) -> Result<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(LauncherError::Config("数据目录不能为空".into()));
    }
    let target = PathBuf::from(trimmed);
    if !target.is_absolute() {
        return Err(LauncherError::Config("请选择完整的绝对路径".into()));
    }
    std::fs::create_dir_all(&target)
        .map_err(|e| LauncherError::Config(format!("无法创建目录（{e}）")))?;
    let target = clean_path(
        std::fs::canonicalize(&target)
            .map_err(|e| LauncherError::Config(format!("无效的目录（{e}）")))?,
    );

    let current = get();
    let old_root =
        clean_path(std::fs::canonicalize(&current.root).unwrap_or_else(|_| current.root.clone()));
    let new_root = resolve_root(&target);

    if new_root == old_root {
        return Ok(new_root);
    }
    // 互相嵌套时整目录搬迁会自己吞自己，直接拒绝
    if new_root.starts_with(&old_root) || old_root.starts_with(&new_root) {
        return Err(LauncherError::Config(
            "新目录不能与当前数据目录互相包含，请换一个位置".into(),
        ));
    }

    migrate(&old_root, &new_root)?;
    write_bootstrap(&current.default_root, Some(&target))?;
    Ok(new_root)
}

/// 恢复默认数据目录（不搬数据，旧目录里的文件保持原样）
pub fn reset_data_dir() -> Result<PathBuf> {
    let current = get();
    let default_root = current.default_root.clone();
    if read_bootstrap(&default_root).is_none() {
        return Ok(current.root.clone());
    }
    write_bootstrap(&default_root, None)?;
    let root = resolve_root(&default_root);
    migrate_legacy(&default_root, &root);
    ensure_dirs(&root)
        .map_err(|e| LauncherError::Config(format!("创建默认目录失败（{e}）")))?;
    Ok(root)
}

fn migrate(old: &Path, new: &Path) -> Result<()> {
    for name in MIGRATE_ENTRIES {
        let src = old.join(name);
        if !src.exists() {
            continue;
        }
        move_entry(&src, &new.join(name))
            .map_err(|e| LauncherError::Message(format!("迁移 {name} 失败（{e}）")))?;
    }
    Ok(())
}

/// 同盘用 rename（瞬时完成），跨盘退化为复制后删除
fn move_entry(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !src.exists() {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_entry(src, dst)?;
    if src.is_dir() {
        std::fs::remove_dir_all(src)?;
    } else {
        std::fs::remove_file(src)?;
    }
    Ok(())
}

fn copy_entry(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_entry(&entry.path(), &dst.join(entry.file_name()))?;
        }
    } else if !dst.exists() {
        // 目标已有同名文件时保留目标：用户可能已经手动迁移过一次
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

/// 在 Builder.setup 中调用一次；路径不可用时直接 panic（此时无法运行）
pub fn init(app: &tauri::AppHandle) -> &'static AppPaths {
    PATHS.get_or_init(|| AppPaths::build(app))
}

pub fn get() -> &'static AppPaths {
    PATHS.get().expect("AppPaths 尚未初始化")
}
