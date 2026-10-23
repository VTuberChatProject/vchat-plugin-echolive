//! 保留原始脚本，并按官方默认值、资源值、用户值和宿主连接配置生成覆盖。
use serde_json::{Value, json};

/// 保持公开接口兼容，调用方可继续只提供原始配置脚本。
pub fn generate_config(source: &str) -> String {
    generate_config_with(source, None, &json!({}))
}

/// HTTP 和配置生成操作共用同一覆盖算法；原始脚本保持逐字节前缀。
pub(crate) fn generate_config_with(
    source: &str,
    schema: Option<&Value>,
    overrides: &Value,
) -> String {
    let inferred;
    let schema = if let Some(schema) = schema {
        schema
    } else {
        inferred = source_schema(source);
        &inferred
    };
    if schema["read_only"] == true {
        return source.to_owned();
    }
    // 直接引用的目录可能被外部更新；声明版本变化时等待重新接入，保持原配置结构。
    if let Ok(resource) = crate::schema::parse_declaration(source, "config")
        && let (Some(declared), Some(expected)) =
            (resource.get("data_version"), schema.get("data_version"))
        && declared != expected
    {
        return source.to_owned();
    }
    let fields = schema["fields"].as_array();
    let managed = fields
        .into_iter()
        .flatten()
        .filter_map(|field| {
            let path = field["path"].as_str()?;
            crate::schema::managed_value(path).map(|value| (path.to_owned(), value))
        })
        .collect::<serde_json::Map<_, _>>();
    let defaults = fields
        .into_iter()
        .flatten()
        .filter_map(|field| Some((field["path"].as_str()?.to_owned(), field["default"].clone())))
        .collect::<serde_json::Map<_, _>>();
    let allowed = overrides
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(path, value)| {
            crate::schema::safe_path(path)
                && crate::schema::managed_value(path).is_none()
                && fields.is_some_and(|fs| {
                    fs.iter().any(|field| {
                        field["path"].as_str() == Some(path)
                            && crate::validation::validate_value(field, value).is_ok()
                    })
                })
        })
        .map(|(path, value)| (path.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    let mut result = String::with_capacity(source.len() + 3_000);
    result.push_str(source);
    if !source.ends_with('\n') {
        result.push('\n');
    }
    result.push_str(";\n");
    result.push_str("// VChat 配置覆盖；原始 config.js 保持完整。\nfunction InjectConfig() {\n");
    result.push_str("const defaults = ");
    result.push_str(&serde_json::to_string(&defaults).expect("JSON 默认值序列化"));
    result.push_str(";\nconst overrides = ");
    result.push_str(&serde_json::to_string(&allowed).expect("JSON 用户配置序列化"));
    result.push_str(";\nconst managed = ");
    result.push_str(&serde_json::to_string(&managed).expect("JSON 宿主配置序列化"));
    result.push_str(r#";
    const own = (node, key) => Object.prototype.hasOwnProperty.call(node, key);
    const set = (path, value, onlyMissing = false) => {
        const keys = path.split('.');
        if (keys.some(key => ['__proto__', 'prototype', 'constructor'].includes(key))) return;
        let node = config;
        for (const key of keys.slice(0, -1)) {
            if (!own(node, key) || node[key] === null || typeof node[key] !== 'object' || Array.isArray(node[key])) node[key] = {};
            node = node[key];
        }
        const key = keys[keys.length - 1];
        if (!onlyMissing || !own(node, key)) node[key] = value;
    };
    // 原始资源已执行；仅补齐缺失值，再应用显式用户覆盖。
    for (const [path, value] of Object.entries(defaults)) set(path, value, true);
    for (const [path, value] of Object.entries(overrides)) set(path, value);
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsAddress = `${protocol}//${window.location.host}/ws`;
    const preview = (window.location.search || '').slice(1).split('&').includes('preview=1');
    // 连接覆盖仅选择当前版本定义中的路径，保持历史配置结构与 data_version。
    for (const [path, value] of Object.entries(managed)) {
        const address = path === 'editor.websocket.url' ? wsAddress : wsAddress + (preview ? '?preview=1' : '');
        const isAddress = ['echolive.websocket_url', 'echolive.broadcast.websocket_url', 'editor.websocket.url'].includes(path);
        set(path, isAddress ? address : value);
    }
}
InjectConfig();
"#);
    result
}

/// 独立公开接口没有目录定义时，只识别已知版本并覆盖资源实际声明的宿主字段。
fn source_schema(source: &str) -> Value {
    let Ok(resource) = crate::schema::parse_declaration(source, "config") else {
        return json!({"read_only": true});
    };
    let Some(version) = resource["data_version"]
        .as_u64()
        .filter(|v| (1..=crate::versions::MAX_DATA_VERSION).contains(v))
    else {
        return json!({"read_only": true});
    };
    let mut fields = Vec::new();
    for path in [
        "echolive.broadcast_enable",
        "echolive.websocket_enable",
        "echolive.websocket_url",
        "echolive.broadcast.enable",
        "echolive.broadcast.websocket_enable",
        "echolive.broadcast.websocket_url",
        "editor.websocket.enable",
        "editor.websocket.url",
        "editor.websocket.auto_url",
        "advanced.broadcast.allow_send_duplicate_message",
    ] {
        let historical = if path.starts_with("editor.websocket.") {
            version >= 10
        } else if path.starts_with("echolive.broadcast.") {
            version >= 4
        } else if path.starts_with("echolive.") {
            version <= 3
        } else {
            true
        };
        if historical && let Some(value) = crate::schema::path_value(&resource, path) {
            fields.push(json!({"path": path, "default": value}));
        }
    }
    json!({"read_only": false, "fields": fields})
}
