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
    let fields = schema.and_then(|s| s["fields"].as_array());
    let defaults = fields
        .into_iter()
        .flatten()
        .filter_map(|field| Some((field["path"].as_str()?.to_owned(), field["default"].clone())))
        .collect::<serde_json::Map<_, _>>();
    let allowed = overrides
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(path, _)| {
            crate::schema::safe_path(path)
                && crate::schema::managed_value(path).is_none()
                && fields.is_some_and(|fs| fs.iter().any(|f| f["path"].as_str() == Some(path)))
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
    set('echolive.broadcast.enable', true);
    set('echolive.broadcast.websocket_enable', true);
    set('echolive.broadcast.websocket_url', wsAddress);
    set('editor.websocket.enable', true);
    set('editor.websocket.url', wsAddress);
    set('editor.websocket.auto_url', false);
}
InjectConfig();
"#);
    result
}
