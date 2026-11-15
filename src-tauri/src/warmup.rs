/*
 * AirLauncher
 * A Tauri-based launcher for the AirPlus Minecraft client.
 * https://github.com/lmx0721/AirPlus
 *
 * 启动预热：启动器空闲几秒后，后台把「正式启动」的前几步（Java 解析、
 * Forge / 客户端 / 库 / 资源检查）提前跑一遍，预热期间 step 日志被抑制，
 * 用户点「开始游戏」时这些步骤直接命中缓存秒过。
 *
 * 预热只会做「已经确定该做的事」：缺什么下什么（下载进度照常进任务面板），
 * 全部就绪的机器上它几乎零开销。
 */
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use crate::{client, forge, java, mojang, paths};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static TOKEN: std::sync::Mutex<Option<CancellationToken>> = std::sync::Mutex::new(None);

/// 预热期间各模块的 step 日志是否应被抑制
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

fn set_active(v: bool) {
    ACTIVE.store(v, Ordering::Relaxed);
}

/// 用户点了「开始游戏」：立即终止预热，避免预热下载与正式启动下载
/// 写同一个 .part 文件（ensure_* 内部下载没有按目标路径去重）。
pub fn cancel() {
    if let Some(token) = TOKEN.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        token.cancel();
    }
    // 立刻解除日志抑制：正式启动的步骤日志必须能显示出来，
    // 不能等预热任务的 Cancelled 错误慢慢展开
    set_active(false);
}

/// 在 lib.rs setup 里调用：空闲 5 秒后开始预热
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // 给界面渲染与首屏 IPC 让路
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        let token = CancellationToken::new();
        *TOKEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(token.clone());
        set_active(true);

        let result = warm(&app, &token).await;
        set_active(false);
        *TOKEN.lock().unwrap_or_else(|e| e.into_inner()) = None;

        if let Err(e) = result
            && !matches!(e, crate::error::LauncherError::Cancelled)
        {
            eprintln!("预热失败（不影响正常启动）: {e}");
        }
    });
}

async fn warm(app: &AppHandle, token: &CancellationToken) -> crate::error::Result<()> {
    // 1) Java：只做本地解析（内置 JRE / 系统 Java 扫描并写缓存），不联网下载
    java::warm_resolve(app).await;

    // 2) Forge 与客户端：本地检查 + 缺失才下载（Forge maven / GitHub release）
    let forge_ready = forge::is_installed();
    if !forge_ready {
        forge::ensure_installed(app, token).await?;
    }

    // 客户端不存在时不预热下载：首页会弹版本选择，主动下载可能下错版本
    let client_installed = client::find_client_jar(&paths::get().mods_dir).is_some();
    if !client_installed {
        return Ok(());
    }

    // 3) 版本链合并 + 库 / 客户端 jar / 资源
    let profile = mojang::load_merged_profile(token).await?;
    let _ = mojang::ensure_client_jar(app, &profile, token).await?;
    mojang::ensure_libraries(app, &profile, token).await?;
    mojang::ensure_assets(app, &profile, token).await?;
    Ok(())
}
