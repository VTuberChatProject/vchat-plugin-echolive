//! 从官方 GitHub Release 下载正式 ZIP，并强制校验附件 SHA256 摘要。
use crate::resources::{MAX_ARCHIVE, check_cancel, hex_digest};
use reqwest::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::Path, sync::atomic::AtomicBool, time::Duration};

const RELEASE_API: &str = "https://api.github.com/repos/sheep-realms/Echo-Live/releases/latest";

/// 下载地址仅来自固定官方仓库的 Release API，摘要缺失时立即失败。
pub(crate) fn download(
    destination: &Path,
    cancelled: &AtomicBool,
    progress: &impl Fn(&str, u64, Option<u64>),
) -> Result<(String, Option<String>), String> {
    let client = Client::builder()
        .user_agent("VChat-EchoLive/1.0")
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .https_only(true)
        .build()
        .map_err(|e| format!("创建下载客户端: {e}"))?;
    download_from(&client, RELEASE_API, destination, cancelled, progress, true)
}

/// 任务线程持有自己的运行时；取消会丢弃网络 future，卸载前可及时 join。
pub(crate) fn download_from(
    client: &Client,
    api: &str,
    destination: &Path,
    cancelled: &AtomicBool,
    progress: &impl Fn(&str, u64, Option<u64>),
    official: bool,
) -> Result<(String, Option<String>), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("创建资源下载运行时: {e}"))?;
    runtime.block_on(async {
        tokio::select! {
            biased;
            _ = wait_cancelled(cancelled) => Err("资源准备已取消".into()),
            result = fetch(client, api, destination, cancelled, progress, official) => result,
        }
    })
}

async fn wait_cancelled(cancelled: &AtomicBool) {
    while check_cancel(cancelled).is_ok() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn fetch(
    client: &Client,
    api: &str,
    destination: &Path,
    cancelled: &AtomicBool,
    progress: &impl Fn(&str, u64, Option<u64>),
    official: bool,
) -> Result<(String, Option<String>), String> {
    check_cancel(cancelled)?;
    progress("requesting_release", 0, None);
    let mut response = client
        .get(api)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("请求官方最新发行信息: {e}"))?;
    if response
        .content_length()
        .is_some_and(|len| len > 2 * 1024 * 1024)
    {
        return Err("Release API 响应超过 2 MiB".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("读取发行信息: {e}"))?
    {
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err("Release API 响应超过 2 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let release: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("发行信息 JSON 无效: {e}"))?;
    let (url, expected) = release_asset(&release, official)?;
    check_cancel(cancelled)?;
    progress("downloading", 0, None);
    let mut response = client
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("下载 Echo-Live 发行 ZIP: {e}"))?;
    let total = response.content_length();
    if total.is_some_and(|bytes| bytes > MAX_ARCHIVE) {
        return Err("发行 ZIP 超过 256 MiB".into());
    }
    let mut file = fs::File::create(destination).map_err(|e| format!("创建下载临时文件: {e}"))?;
    let mut digest = Sha256::new();
    let mut bytes = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("读取发行 ZIP: {e}"))?
    {
        check_cancel(cancelled)?;
        bytes += chunk.len() as u64;
        if bytes > MAX_ARCHIVE {
            return Err("发行 ZIP 超过 256 MiB".into());
        }
        file.write_all(&chunk)
            .map_err(|e| format!("写入发行 ZIP: {e}"))?;
        digest.update(&chunk);
        progress("downloading", bytes, total);
    }
    file.flush().map_err(|e| format!("刷新发行 ZIP: {e}"))?;
    if bytes == 0 {
        return Err("发行 ZIP 为空".into());
    }
    check_cancel(cancelled)?;
    progress("verifying_sha256", bytes, total);
    let actual = hex_digest(&digest.finalize());
    if actual != expected {
        return Err(format!("SHA256 校验失败；预期 {expected}，实际 {actual}"));
    }
    Ok((actual, release["tag_name"].as_str().map(str::to_owned)))
}

fn release_asset(release: &Value, official: bool) -> Result<(String, String), String> {
    if release["draft"] != false || release["prerelease"] != false {
        return Err("Release API 返回的版本必须为已发布的稳定版本".into());
    }
    let assets = release["assets"].as_array().ok_or("发行信息缺少 assets")?;
    let archives = assets
        .iter()
        .filter(|asset| {
            asset["name"].as_str().is_some_and(|name| {
                let lower = name.to_ascii_lowercase();
                lower.starts_with("echo-live-")
                    && lower.ends_with(".zip")
                    && !lower.contains("source")
            })
        })
        .collect::<Vec<_>>();
    if archives.len() != 1 {
        return Err(format!(
            "最新发行需要唯一的 Echo-Live 正式 ZIP 附件，找到 {} 个",
            archives.len()
        ));
    }
    let asset = archives[0];
    if asset["size"].as_u64().is_some_and(|len| len > MAX_ARCHIVE) {
        return Err("发行 ZIP 超过 256 MiB".into());
    }
    let digest = asset["digest"]
        .as_str()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or("官方发行附件缺少可信的 SHA256 digest；请使用文件夹或导入 ZIP")?
        .to_ascii_lowercase();
    let url = asset["browser_download_url"]
        .as_str()
        .ok_or("发行附件缺少下载地址")?;
    if official && !url.starts_with("https://github.com/sheep-realms/Echo-Live/releases/download/")
    {
        return Err("发行附件地址超出官方仓库下载路径".into());
    }
    Ok((url.to_owned(), digest))
}
