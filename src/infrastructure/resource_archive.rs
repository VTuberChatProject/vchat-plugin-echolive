//! ZIP 归档的有界复制、安全解压和资源根定位。
use crate::resources::{MAX_ARCHIVE, MAX_EXPANDED, MAX_FILE, check_cancel};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

/// 流式计算归档 SHA256，同时限制输入大小并响应取消。
pub(crate) fn copy_archive(
    source: &mut impl Read,
    target: &mut impl Write,
    cancelled: &AtomicBool,
    progress: &impl Fn(&str, u64, Option<u64>),
) -> Result<String, String> {
    let mut digest = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check_cancel(cancelled)?;
        let count = source
            .read(&mut buffer)
            .map_err(|e| format!("读取归档: {e}"))?;
        if count == 0 {
            break;
        }
        bytes = bytes.checked_add(count as u64).ok_or("压缩包长度溢出")?;
        if bytes > MAX_ARCHIVE {
            return Err("压缩包超过 256 MiB 限制".into());
        }
        target
            .write_all(&buffer[..count])
            .map_err(|e| format!("写入临时归档: {e}"))?;
        digest.update(&buffer[..count]);
        progress("transferring", bytes, None);
    }
    target.flush().map_err(|e| format!("刷新临时归档: {e}"))?;
    if bytes == 0 {
        return Err("压缩包为空".into());
    }
    Ok(crate::resources::hex_digest(&digest.finalize()))
}

/// 在新建暂存目录解压，逐项拒绝路径穿越、设备名、链接及大小超限。
pub(crate) fn extract(
    archive: &Path,
    destination: &Path,
    cancelled: &AtomicBool,
    progress: &impl Fn(&str, u64, Option<u64>),
) -> Result<(), String> {
    let input = fs::File::open(archive).map_err(|e| format!("打开 ZIP: {e}"))?;
    let mut zip = zip::ZipArchive::new(input).map_err(|e| format!("ZIP 格式损坏: {e}"))?;
    if zip.is_empty() || zip.len() > 30_000 {
        return Err("ZIP 条目数量必须为 1～30000".into());
    }
    fs::create_dir(destination).map_err(|e| format!("创建解压目录: {e}"))?;
    let mut names = HashSet::new();
    let mut declared = 0u64;
    let mut actual = 0u64;
    let entries = zip.len();
    for index in 0..entries {
        check_cancel(cancelled)?;
        let mut entry = zip
            .by_index(index)
            .map_err(|e| format!("读取 ZIP 条目 {index}: {e}"))?;
        let name = entry.name().to_owned();
        let relative = safe_name(&name)?;
        let collision_key = relative.to_string_lossy().replace('\\', "/").to_lowercase();
        if !names.insert(collision_key) {
            return Err(format!("ZIP 路径重复：{name}"));
        }
        if entry.encrypted() || entry.is_symlink() {
            return Err(format!("ZIP 条目为加密文件或链接：{name}"));
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o040000 | 0o100000))
        {
            return Err(format!("ZIP 条目类型无效：{name}"));
        }
        if entry.size() > MAX_FILE {
            return Err(format!("ZIP 单文件超过 128 MiB：{name}"));
        }
        declared = declared
            .checked_add(entry.size())
            .ok_or("ZIP 展开长度溢出")?;
        if declared > MAX_EXPANDED {
            return Err("ZIP 展开大小超过 1 GiB".into());
        }
        let output = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&output).map_err(|e| format!("创建目录 {name}: {e}"))?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建上级目录 {name}: {e}"))?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|e| format!("创建文件 {name}: {e}"))?;
        let mut file_bytes = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_cancel(cancelled)?;
            let count = entry
                .read(&mut buffer)
                .map_err(|e| format!("解压文件 {name}: {e}"))?;
            if count == 0 {
                break;
            }
            file_bytes += count as u64;
            actual += count as u64;
            if file_bytes > MAX_FILE || actual > MAX_EXPANDED || file_bytes > entry.size() {
                return Err(format!("ZIP 实际展开大小超限：{name}"));
            }
            file.write_all(&buffer[..count])
                .map_err(|e| format!("写入文件 {name}: {e}"))?;
        }
        if file_bytes != entry.size() {
            return Err(format!("ZIP 文件长度不匹配：{name}"));
        }
        progress("extracting", (index + 1) as u64, Some(entries as u64));
    }
    Ok(())
}

fn safe_name(name: &str) -> Result<PathBuf, String> {
    let invalid = || format!("ZIP 路径无效或越界：{name}");
    if name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.chars().any(|c| c.is_control())
    {
        return Err(invalid());
    }
    let mut path = PathBuf::new();
    for component in name.trim_end_matches('/').split('/') {
        if component.is_empty()
            || matches!(component, "." | "..")
            || component.ends_with([' ', '.'])
            || component.contains([':', '*', '?', '"', '<', '>', '|'])
        {
            return Err(invalid());
        }
        let base = component
            .split('.')
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        if matches!(
            base.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && matches!(base.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(invalid());
        }
        path.push(component);
    }
    Ok(path)
}

/// 支持根目录及单层包装；多个候选根要求用户提供明确的发行包。
pub(crate) fn find_root(directory: &Path) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if directory.join("config.js").is_file() && directory.join("live.html").is_file() {
        candidates.push(directory.to_path_buf());
    }
    let children = fs::read_dir(directory).map_err(|e| format!("读取解压目录: {e}"))?;
    for entry in children {
        let entry = entry.map_err(|e| format!("读取解压条目: {e}"))?;
        let child = entry.path();
        if entry.file_type().map_err(|e| e.to_string())?.is_dir()
            && child.join("config.js").is_file()
            && child.join("live.html").is_file()
        {
            candidates.push(child);
        }
    }
    if candidates.len() > 1 {
        candidates.retain(|candidate| crate::resources::validate(candidate).is_ok());
    }
    match candidates.len() {
        1 => Ok(candidates.remove(0)),
        0 => Err(
            "ZIP 中未找到 Echo-Live 资源根；需要根目录或单层文件夹中的 config.js 和 live.html"
                .into(),
        ),
        _ => Err("ZIP 中存在多个 Echo-Live 资源根，请导入单个正式发行目录".into()),
    }
}
