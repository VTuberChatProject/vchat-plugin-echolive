//! 摘要目录复用前比较完整文件树，防止外部修改后继续沿用原归档身份。
use crate::resources::{MAX_EXPANDED, MAX_FILE, check_cancel, hex_digest};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

/// 候选树来自本次已验证归档；复用树必须保持完全相同的路径、类型和内容。
pub(crate) fn verify_reuse(
    candidate: &Path,
    existing: &Path,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let expected = manifest(candidate, cancelled)?;
    let actual = manifest(existing, cancelled)?;
    if expected != actual {
        let changed = expected
            .iter()
            .find(|(path, value)| actual.get(*path) != Some(*value))
            .map(|(path, _)| path)
            .or_else(|| actual.keys().find(|path| !expected.contains_key(*path)));
        return Err(format!(
            "已有摘要目录内容已被外部修改，无法复用；差异路径：{}。请保留现有资源并选择其他资源目录",
            changed.map_or_else(|| "未知".into(), |path| path.display().to_string())
        ));
    }
    Ok(())
}

fn manifest(
    root: &Path,
    cancelled: &AtomicBool,
) -> Result<BTreeMap<PathBuf, Option<String>>, String> {
    check_cancel(cancelled)?;
    reject_link(
        root,
        &fs::symlink_metadata(root).map_err(|e| format!("读取资源根类型: {e}"))?,
    )?;
    let mut files = BTreeMap::new();
    let mut directories = vec![PathBuf::new()];
    let mut expanded = 0u64;
    while let Some(relative) = directories.pop() {
        check_cancel(cancelled)?;
        for entry in fs::read_dir(root.join(&relative)).map_err(|e| format!("读取资源清单: {e}"))?
        {
            check_cancel(cancelled)?;
            let entry = entry.map_err(|e| format!("读取资源条目: {e}"))?;
            let path = entry.path();
            let relative = relative.join(entry.file_name());
            let metadata = fs::symlink_metadata(&path)
                .map_err(|e| format!("读取资源类型 {}: {e}", relative.display()))?;
            reject_link(&path, &metadata)?;
            if files.len() >= 30_000 {
                return Err("资源目录条目超过 30000，无法复用".into());
            }
            if metadata.is_dir() {
                files.insert(relative.clone(), None);
                directories.push(relative);
            } else if metadata.is_file() {
                if metadata.len() > MAX_FILE {
                    return Err(format!("资源文件超过 128 MiB：{}", relative.display()));
                }
                let mut input = fs::File::open(&path)
                    .map_err(|e| format!("打开资源 {}: {e}", relative.display()))?;
                let mut digest = Sha256::new();
                let mut buffer = [0u8; 64 * 1024];
                let mut length = 0u64;
                loop {
                    check_cancel(cancelled)?;
                    let count = input
                        .read(&mut buffer)
                        .map_err(|e| format!("计算资源摘要 {}: {e}", relative.display()))?;
                    if count == 0 {
                        break;
                    }
                    length += count as u64;
                    expanded += count as u64;
                    if length > MAX_FILE || expanded > MAX_EXPANDED {
                        return Err("已有摘要目录内容超限，无法复用".into());
                    }
                    digest.update(&buffer[..count]);
                }
                if length != metadata.len() {
                    return Err(format!(
                        "资源文件在校验过程中被修改：{}",
                        relative.display()
                    ));
                }
                files.insert(relative, Some(hex_digest(&digest.finalize())));
            } else {
                return Err(format!("资源目录包含特殊文件：{}", relative.display()));
            }
        }
    }
    Ok(files)
}

fn reject_link(path: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    let link = metadata.file_type().is_symlink();
    #[cfg(windows)]
    let link = {
        use std::os::windows::fs::MetadataExt;
        link || metadata.file_attributes() & 0x400 != 0
    };
    if link {
        Err(format!(
            "摘要目录包含链接或重解析点，无法复用：{}",
            path.display()
        ))
    } else {
        Ok(())
    }
}
