//! 按官方默认值、资源值、用户值和宿主连接配置生成完整运行时配置输出。
use serde_json::{Value, json};

/// 保持公开接口兼容，调用方可继续只提供原始配置脚本。
pub fn generate_config(source: &str) -> String {
    generate_config_with(source, None, &json!({}))
}

/// HTTP 和配置生成操作共用同一覆盖算法；输出使用完整配置声明替换原始脚本。
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
    let mut config = match crate::schema::parse_declaration(source, "config") {
        Ok(value) if value.is_object() => value,
        _ => return source.to_owned(),
    };
    for (path, value) in &defaults {
        set_path(&mut config, path, value.clone(), true);
    }
    for (path, value) in &allowed {
        set_path(&mut config, path, value.clone(), false);
    }
    for (path, value) in &managed {
        set_path(&mut config, path, value.clone(), false);
    }
    // 宿主连接地址依赖当前页面位置，保留极小的运行时赋值段；其它配置已完整替换。
    let json = serde_json::to_string_pretty(&config).expect("JSON 配置序列化");
    let mut result = format!("const config = {json};\n");
    result.push_str("// VChat 运行时连接配置。\nfunction InjectConfig() {\n");
    result.push_str("const managed = ");
    result.push_str(&serde_json::to_string(&managed).expect("JSON 宿主管理配置序列化"));
    result.push_str(";\n");
    result.push_str(r#"
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
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsAddress = `${protocol}//${window.location.host}/ws`;
    const preview = (window.location.search || '').slice(1).split('&').includes('preview=1');
    const hasPath = (path) => {
        let node = config;
        for (const key of path.split('.')) { if (!own(node, key)) return false; node = node[key]; }
        return true;
    };
    for (const [path, value] of Object.entries(managed)) {
        if (!hasPath(path)) continue;
        const address = path === 'editor.websocket.url' ? wsAddress : wsAddress + (preview ? '?preview=1' : '');
        const isAddress = ['echolive.websocket_url', 'echolive.broadcast.websocket_url', 'editor.websocket.url'].includes(path);
        set(path, isAddress ? address : value);
    }
}
InjectConfig();
"#);
    result
}

fn set_path(root: &mut Value, path: &str, value: Value, only_missing: bool) {
    let mut node = root;
    let mut parts = path.split('.').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            if !only_missing || !node.get(part).is_some() {
                node[part] = value;
            }
            return;
        }
        if !node.get(part).is_some_and(Value::is_object) {
            node[part] = Value::Object(Default::default());
        }
        node = &mut node[part];
    }
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
