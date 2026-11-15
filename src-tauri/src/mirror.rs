/*
 * AirLauncher
 * A Tauri-based launcher for the AirPlus Minecraft client.
 * https://github.com/lmx0721/AirPlus
 *
 * GitHub 下载加速：直连 + 公共镜像并发探速，取最快的一条。
 *
 * 国内直连 github.com 的 release 附件经常只有几十 KB/s，甚至直接超时；
 * 公共加速镜像（gh-proxy 等）走的是境外 CDN 回源 + 国内节点，通常快一个数量级。
 * 这里在真正下载前用 Range 请求各拉一小段做测速，选中最快的地址，
 * 并对结果做进程内缓存，避免同一 URL 反复测速。
 */
use crate::net;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 每次探速最多读取的字节数（越大越准，但对小文件开销也越大）
const PROBE_BYTES: usize = 256 * 1024;
/// 单个候选地址的探速超时
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 公共加速镜像前缀，用法是直接拼在完整原始 URL 前面：
/// `https://gh-proxy.com/https://github.com/owner/repo/releases/download/...`
const MIRRORS: &[&str] = &[
    "https://gh-proxy.com/",
    "https://ghfast.top/",
    "https://ghproxy.net/",
];

fn cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, String>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 只对 GitHub 的 release 附件做加速；其它源（Mojang / Modrinth 等）原样返回
pub fn is_github(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.starts_with("https://github.com/")
        || lower.starts_with("https://objects.githubusercontent.com/")
        || lower.starts_with("https://raw.githubusercontent.com/")
}

fn candidates(url: &str) -> Vec<String> {
    let mut list = Vec::with_capacity(MIRRORS.len() + 1);
    list.push(url.to_string());
    for m in MIRRORS {
        list.push(format!("{m}{url}"));
    }
    list
}

/// 选出实际下载用的地址（已缓存）
pub async fn resolve(url: &str) -> String {
    if !is_github(url) {
        return url.to_string();
    }
    if let Some(hit) = cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(url)
        .cloned()
    {
        return hit;
    }

    let list = candidates(url);
    let probed = futures_util::future::join_all(list.iter().map(|c| probe(c))).await;

    let mut best: Option<(u64, &String)> = None;
    for (cand, score) in list.iter().zip(probed) {
        // 直连在测速里略占优势：分数接近时优先直连，少一层中转
        let weighted = match score {
            Some(s) => s,
            None => continue,
        };
        if best.is_none_or(|(b, _)| weighted > b) {
            best = Some((weighted, cand));
        }
    }

    let chosen = best
        .map(|(_, c)| c.clone())
        .unwrap_or_else(|| url.to_string());
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(url.to_string(), chosen.clone());
    chosen
}

/// 下载失败后调用：清掉缓存，下次重试会重新测速（可能换一条更稳的线路）
pub fn forget(url: &str) {
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(url);
}

/// 拉一小段测吞吐，返回 bytes/s；失败或超时返回 None
async fn probe(url: &str) -> Option<u64> {
    let fut = async {
        let resp = net::get()
            .get(url)
            .header("Range", format!("bytes=0-{}", PROBE_BYTES - 1))
            .header("Accept-Encoding", "identity")
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?;

        let started = Instant::now();
        let mut stream = resp.bytes_stream();
        let mut got = 0usize;
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else { break };
            got += chunk.len();
            if got >= PROBE_BYTES {
                break;
            }
        }
        if got == 0 {
            return None;
        }
        let secs = started.elapsed().as_secs_f64().max(0.001);
        Some(((got as f64 / secs) as u64).max(1))
    };
    tokio::time::timeout(PROBE_TIMEOUT, fut)
        .await
        .ok()
        .flatten()
}
