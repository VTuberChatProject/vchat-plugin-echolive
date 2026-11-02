//! Echo-Live 外置资源的静态完整性校验及可回滚的准备入口。
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Component, Path},
    sync::atomic::{AtomicBool, Ordering},
};

pub(crate) const MAX_ARCHIVE: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_FILE: u64 = 128 * 1024 * 1024;
pub(crate) const MAX_EXPANDED: u64 = 1024 * 1024 * 1024;

pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn check_cancel(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) {
        Err("资源准备已取消".into())
    } else {
        Ok(())
    }
}

/// 读取有界文本，静态解析流程始终保留上游 JavaScript 原样。
pub(crate) fn read_text(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 16 * 1024 * 1024 {
        return Err(format!(
            "{}: 文件为空、类型错误或超过 16 MiB",
            path.display()
        ));
    }
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// 依据真实页面依赖校验历史发行目录，返回配置及播放能力描述。
pub(crate) fn validate(root: &Path) -> Result<Value, String> {
    let root = fs::canonicalize(root).map_err(|e| format!("资源目录 {}: {e}", root.display()))?;
    if !root.is_dir() {
        return Err("Echo-Live 资源路径必须为目录".into());
    }
    let mut problems = Vec::new();
    for directory in ["res", "lib"] {
        if !root.join(directory).is_dir() {
            problems.push(format!("缺少目录 {directory}"));
        }
    }
    validate_reference(&root, "config.js")?;
    let source = read_text(&root.join("config.js"))?;
    let config = crate::schema::parse_declaration(&source, "config")
        .map_err(|e| format!("config.js: {e}"))?;
    if !config.is_object() {
        return Err("config.js: config 必须为对象".into());
    }
    let version = config["data_version"].as_u64();
    let schema_file = version.and_then(|version| crate::versions::definition_path(&root, version));
    if version.is_some_and(|v| (2..=17).contains(&v)) && schema_file.is_none() {
        problems.push("缺少当前版本配置字段定义 config_define.js / config-define.js".into());
    }
    if version.is_some_and(|v| (2..=17).contains(&v)) {
        if let Err(error) = validate_reference(&root, "lang/zho-Hans.js")
            .and_then(|_| read_text(&root.join("lang/zho-Hans.js")).map(|_| ()))
        {
            problems.push(error);
        }
        if let Some(file) = schema_file
            && let Err(error) = validate_reference(&root, file)
                .and_then(|_| read_text(&root.join(file)).map(|_| ()))
        {
            problems.push(error);
        }
    }
    let mut capabilities = serde_json::Map::new();
    for page in ["live", "editor", "history", "character", "settings"] {
        let file = format!("{page}.html");
        let exists = root.join(&file).is_file();
        capabilities.insert(page.into(), json!(exists));
        if page == "live" && !exists {
            problems.push(format!("缺少页面 {file}"));
        }
        if exists {
            if let Err(error) = validate_reference(&root, &file) {
                problems.push(error);
                continue;
            }
            match read_text(&root.join(&file)) {
                Ok(html) if html.to_ascii_lowercase().contains("<html") => {
                    let dependencies = html_dependencies(&html);
                    if page == "live" {
                        for required in [
                            "config.js",
                            "res/class/Echo.js",
                            "res/class/EchoLive.js",
                            "res/class/EchoLiveBroadcast.js",
                            "res/script/live.js",
                        ] {
                            if !dependencies.iter().any(|value| {
                                value.trim_start_matches("./").split(['?', '#']).next()
                                    == Some(required)
                            }) {
                                problems.push(format!("live.html: 缺少运行入口引用 {required}"));
                            }
                            if let Err(error) = validate_reference(&root, required) {
                                problems.push(error);
                            }
                        }
                    }
                    for reference in dependencies {
                        if let Err(error) = validate_reference(&root, &reference) {
                            problems.push(format!("{file}: {error}"));
                        }
                    }
                }
                Ok(_) => problems.push(format!("{file}: HTML 内容无效")),
                Err(error) => problems.push(error),
            }
        }
    }
    if !problems.is_empty() {
        return Err(format!("Echo-Live 目录校验失败：\n{}", problems.join("\n")));
    }
    let schema = crate::schema::load_schema(&root)
        .map_err(|error| format!("Echo-Live 配置元数据校验失败：{error}"))?;
    let bridge = [
        (
            "res/class/EchoLiveBroadcast.js",
            &[
                "class EchoLiveBroadcastPortal",
                "getDataPortal",
                "echoPrinting",
                "echoStateUpdate",
                "API_NAME_ECHO_PRINTING",
                "API_NAME_ECHO_STATE_UPDATE",
            ][..],
        ),
        (
            "res/class/EchoLive.js",
            &["clearTask(", "clearDisplayHiddenWaitTimer(", "send("][..],
        ),
        (
            "res/class/Echo.js",
            &["createSubTrack(", "next(", "print(", "on(", "stop("][..],
        ),
    ]
    .iter()
    .all(|(file, symbols)| {
        read_text(&root.join(file))
            .is_ok_and(|text| symbols.iter().all(|symbol| text.contains(symbol)))
    });
    capabilities.insert("bridge".into(), json!(bridge));
    let compatibility = match version {
        Some(1..=17) => "supported",
        Some(_) => "unsupported_version",
        None => "missing_version",
    };
    Ok(
        json!({"path": root, "data_version": version, "schema_version": schema["schema_version"],
        "compatibility_status": compatibility, "compatibility": schema["compatibility"],
        "read_only": schema["read_only"], "capabilities": capabilities}),
    )
}

fn validate_reference(root: &Path, reference: &str) -> Result<(), String> {
    let reference = reference.trim().split(['?', '#']).next().unwrap_or("");
    if reference.is_empty()
        || reference.starts_with("//")
        || reference.contains("://")
        || reference.starts_with("data:")
        || reference.starts_with("blob:")
    {
        return Ok(());
    }
    let local = decode_url(reference)?;
    let path = Path::new(local.trim_start_matches('/'));
    if local.contains('\\')
        || local.contains(':')
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(format!("依赖路径越界：{reference}"));
    }
    let dependency =
        fs::canonicalize(root.join(path)).map_err(|e| format!("缺少依赖 {reference}: {e}"))?;
    if !dependency.starts_with(root) {
        return Err(format!("依赖指向目录外部：{reference}"));
    }
    let metadata = fs::metadata(&dependency).map_err(|e| format!("依赖 {reference}: {e}"))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(format!("依赖文件无效：{reference}"));
    }
    Ok(())
}

fn decode_url(value: &str) -> Result<String, String> {
    let mut bytes = Vec::new();
    let mut cursor = value.bytes();
    while let Some(byte) = cursor.next() {
        if byte == b'%' {
            let hex = [cursor.next(), cursor.next()];
            let [Some(a), Some(b)] = hex else {
                return Err(format!("依赖 URL 编码无效：{value}"));
            };
            let digits = [a, b];
            bytes.push(
                u8::from_str_radix(
                    std::str::from_utf8(&digits).map_err(|_| "依赖 URL 编码无效")?,
                    16,
                )
                .map_err(|_| format!("依赖 URL 编码无效：{value}"))?,
            );
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).map_err(|_| format!("依赖 URL 编码无效：{value}"))
}

/// 仅提取静态运行标签的属性，兼容单双引号及无引号路径。
fn html_dependencies(source: &str) -> Vec<String> {
    let lower = source.to_ascii_lowercase();
    let mut references = Vec::new();
    let mut offset = 0;
    while let Some(index) = lower[offset..].find('<') {
        let start = offset + index;
        if lower[start..].starts_with("<!--") {
            offset = lower[start..]
                .find("-->")
                .map_or(lower.len(), |n| start + n + 3);
            continue;
        }
        let Some(end) = lower[start..].find('>') else {
            break;
        };
        offset = start + end + 1;
        let tag = &source[start + 1..start + end];
        let name = tag
            .split_ascii_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let attribute = match name.as_str() {
            "script" | "img" | "source" | "audio" | "video" => "src",
            "link" => "href",
            _ => continue,
        };
        let bytes = tag.as_bytes();
        let mut i = name.len();
        while i < bytes.len() {
            while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            let key = i;
            while bytes
                .get(i)
                .is_some_and(|b| !b.is_ascii_whitespace() && !matches!(b, b'=' | b'/' | b'>'))
            {
                i += 1;
            }
            let key_name = &tag[key..i];
            while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            if bytes.get(i) != Some(&b'=') {
                i += 1;
                continue;
            }
            i += 1;
            while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            let quote = bytes.get(i).copied().filter(|b| matches!(b, b'\'' | b'"'));
            if quote.is_some() {
                i += 1;
            }
            let value = i;
            while bytes
                .get(i)
                .is_some_and(|b| quote.map_or(!b.is_ascii_whitespace(), |q| *b != q))
            {
                i += 1;
            }
            if key_name.eq_ignore_ascii_case(attribute) {
                references.push(tag[value..i].replace("&amp;", "&"));
            }
            if quote.is_some() {
                i += 1;
            }
        }
    }
    references
}

/// 文件夹直接引用；导入与下载在完整验证后原子发布到摘要目录。
pub(crate) fn prepare(
    assets: &Path,
    action: &str,
    path: Option<&Path>,
    cancelled: &AtomicBool,
    progress: impl Fn(&str, u64, Option<u64>),
) -> Result<Value, String> {
    check_cancel(cancelled)?;
    if action == "folder" {
        progress("validating", 0, None);
        let mut descriptor = validate(path.ok_or("请选择 Echo-Live 文件夹")?)?;
        check_cancel(cancelled)?;
        let canonical = descriptor["path"].as_str().ok_or("资源目录路径无效")?;
        descriptor["id"] = json!(format!(
            "folder-{}",
            hex_digest(&Sha256::digest(canonical.as_bytes()))
        ));
        descriptor["source"] = json!("folder");
        descriptor["sha256"] = Value::Null;
        return Ok(descriptor);
    }
    if !matches!(action, "import" | "download") {
        return Err(format!("未知资源获取操作：{action}"));
    }
    let base = assets.join("Echo-Live");
    fs::create_dir_all(&base).map_err(|e| format!("创建 Assets/Echo-Live: {e}"))?;
    let staging = tempfile::Builder::new()
        .prefix(".prepare-")
        .tempdir_in(&base)
        .map_err(|e| e.to_string())?;
    let archive = staging.path().join("release.zip");
    let (digest, release) = if action == "download" {
        crate::resource_download::download(&archive, cancelled, &progress)?
    } else {
        let input = path.ok_or("请选择 Echo-Live ZIP 压缩包")?;
        let mut source = fs::File::open(input).map_err(|e| format!("打开压缩包: {e}"))?;
        let mut target = fs::File::create(&archive).map_err(|e| e.to_string())?;
        progress("copying", 0, source.metadata().ok().map(|v| v.len()));
        (
            crate::resource_archive::copy_archive(&mut source, &mut target, cancelled, &progress)?,
            None,
        )
    };
    check_cancel(cancelled)?;
    progress("extracting", 0, None);
    let content = staging.path().join("content");
    crate::resource_archive::extract(&archive, &content, cancelled, &progress)?;
    progress("validating", 0, None);
    let candidate = crate::resource_archive::find_root(&content)?;
    validate(&candidate)?;
    check_cancel(cancelled)?;
    let destination = base.join(&digest);
    if destination.exists() {
        validate(&destination).map_err(|e| format!("已存在的摘要目录校验失败：{e}"))?;
        progress("verifying_reuse", 0, None);
        crate::resource_manifest::verify_reuse(&candidate, &destination, cancelled)?;
    } else {
        fs::rename(&candidate, &destination).map_err(|e| format!("发布资源目录: {e}"))?;
    }
    let mut descriptor = validate(&destination)?;
    descriptor["id"] = json!(digest);
    descriptor["sha256"] = descriptor["id"].clone();
    descriptor["source"] = json!(action);
    if let Some(release) = release {
        descriptor["release"] = json!(release);
    }
    progress("ready", 0, None);
    Ok(descriptor)
}
