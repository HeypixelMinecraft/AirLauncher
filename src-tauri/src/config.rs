/*
 * AirLauncher
 * A Tauri-based launcher for the AirPlus Minecraft client.
 * https://github.com/lmx0721/AirPlus
 */
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// 窗口效果: "acrylic" | "mica" | "none"
    pub window_effect: String,
    /// 游戏最大内存 (MB)
    pub ram_mb: u32,
    /// 额外 JVM 参数
    pub extra_jvm_args: Vec<String>,
    /// 下载并发数
    pub download_concurrency: u32,
    /// GitHub PAT（可选，提升 API 限额）
    pub github_pat: Option<String>,
    /// 下载镜像（可选，如 BMCLAPI 前缀），None 为官方源
    pub mirror: Option<String>,
    /// 当前选中的微软账号 id（accounts.json 中的账号）
    pub active_account_id: Option<String>,
    /// 用户手动指定的 Java 路径（最高优先级，设置页「浏览…」选择）
    pub java_path: Option<String>,
    /// 上次解析出的 Java 来源："manual" | "bundled" | "system" | "download"
    /// 持久化检测结果，避免每次冷启动都重新扫描系统 Java
    pub java_source: Option<String>,
    /// 上次找到的系统 Java 路径（javaSource = "system" 时有效）
    pub system_java_path: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            window_effect: "acrylic".to_string(),
            ram_mb: 2048,
            extra_jvm_args: Vec::new(),
            download_concurrency: 8,
            github_pat: None,
            mirror: None,
            active_account_id: None,
            java_path: None,
            java_source: None,
            system_java_path: None,
        }
    }
}

impl Settings {
    pub fn load(file: &Path) -> Settings {
        std::fs::read(file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// 原子写：先写 .tmp 再 rename，避免写一半损坏配置
    pub fn save(&self, file: &Path) -> Result<()> {
        let tmp = file.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(self)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, file)?;
        Ok(())
    }
}
