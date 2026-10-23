//! 静态读取 Echo-Live 历史字段定义、中文标签与资源配置。
use serde_json::{Value, json};
use std::path::Path;

/// VChat 持有的连接字段始终以页面同源地址生成。
pub(crate) fn managed_value(path: &str) -> Option<Value> {
    match path {
        "echolive.broadcast_enable"
        | "echolive.websocket_enable"
        | "echolive.broadcast.enable"
        | "echolive.broadcast.websocket_enable"
        | "advanced.broadcast.allow_send_duplicate_message"
        | "editor.websocket.enable" => Some(json!(true)),
        "editor.websocket.auto_url" => Some(json!(false)),
        "echolive.websocket_url" | "echolive.broadcast.websocket_url" | "editor.websocket.url" => {
            Some(json!("由 VChat 按当前页面地址生成"))
        }
        _ => None,
    }
}

/// 拒绝原型链键，保证数据路径只能访问配置自身的普通属性。
pub(crate) fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('.').all(|part| {
            !part.is_empty()
                && !matches!(part, "__proto__" | "prototype" | "constructor")
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
}

/// 用点路径读取嵌套官方配置。
pub(crate) fn path_value<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(root, |node, part| node.get(part))
}

/// 加载单份管理定义；错误包含资源文件和 JSON5 行列位置。
pub(crate) fn load_schema(root: &Path) -> Result<Value, String> {
    let resource = read_declaration(root, "config.js", "config")?;
    if !resource.is_object() {
        return Err("config.js: config 必须为对象".into());
    }
    let version_value = resource.get("data_version").cloned().unwrap_or(Value::Null);
    let Some(version) = version_value
        .as_u64()
        .filter(|v| (1..=crate::versions::MAX_DATA_VERSION).contains(v))
    else {
        return Ok(crate::versions::read_only_schema(&version_value));
    };
    let (definitions, language, schema_version) = if version == 1 {
        (crate::versions::v1_definitions(), json!({}), Some(1))
    } else {
        let file = crate::versions::definition_path(root, version).ok_or_else(|| {
            format!("data_version {version}: 缺少官方 config_define.js / config-define.js 定义")
        })?;
        let source = crate::resources::read_text(&root.join(file))?;
        let definitions =
            parse_declaration(&source, "db_config_define").map_err(|e| format!("{file}: {e}"))?;
        let schema_version = declaration_number(&source, "db_config_version");
        let language = read_declaration(root, "lang/zho-Hans.js", "lang_zho_Hans")?;
        (definitions, language, schema_version)
    };
    let effective = crate::versions::effective_version(version, schema_version);
    let mut fields = Vec::new();
    let mut paths = std::collections::HashMap::new();
    for definition in definitions
        .as_array()
        .ok_or("config_define.js: 定义必须为数组")?
    {
        if definition["type"] == "object" || !crate::versions::active(definition, effective) {
            continue;
        }
        let path = crate::versions::field_path(definition, effective)
            .ok_or("config_define.js: 字段缺少 name")?;
        if !safe_path(path) {
            return Err(format!("config_define.js: 配置路径无效或重复：{path}"));
        }
        if let Some(previous) = paths.insert(path.to_owned(), definition) {
            // v2 官方定义重复列出 editor.log_line_maximum；相同定义折叠为一个控件。
            if previous == definition {
                continue;
            }
            return Err(format!("config_define.js: 配置路径存在冲突定义：{path}"));
        }
        let kind = definition["type"]
            .as_str()
            .ok_or_else(|| format!("{path}: 缺少类型"))?;
        let default = definition
            .get("default")
            .ok_or_else(|| format!("{path}: 缺少默认值"))?;
        let localized = path_value(&language["config"], path);
        let label = localized
            .and_then(|v| v["_title"].as_str())
            .or_else(|| definition["label"].as_str())
            .unwrap_or(path);
        let managed = managed_value(path);
        let value = managed
            .clone()
            .unwrap_or_else(|| path_value(&resource, path).unwrap_or(default).clone());
        let mut field = json!({
            "path": path, "label": label, "group": path.split('.').next().unwrap_or("advanced"),
            "kind": kind, "default": default, "value": value,
            "conditions": definition.get("conditions").cloned().unwrap_or_else(|| json!([])),
            "read_only": managed.is_some(), "from": definition.get("from"),
        });
        for (upstream, name) in [("min", "minimum"), ("max", "maximum"), ("step", "step")] {
            if let Some(value) = definition["attribute"].get(upstream) {
                field[name] = value.clone();
            }
        }
        if let Some(unit) = definition.get("unit") {
            field["unit"] = unit.clone();
        }
        if let Some(input_type) = definition["attribute"].get("type") {
            field["input_type"] = input_type.clone();
        } else if path.ends_with("_color") {
            field["input_type"] = json!("color");
        }
        crate::validation::validate_value(&field, default)?;
        fields.push(field);
    }
    Ok(
        json!({"version": 1, "data_version": version, "schema_version": schema_version,
        "effective_version": effective, "read_only": false,
        "compatibility": crate::versions::compatibility(Some(version), Some(effective)), "fields": fields}),
    )
}

fn read_declaration(root: &Path, file: &str, name: &str) -> Result<Value, String> {
    let source = crate::resources::read_text(&root.join(file))?;
    parse_declaration(&source, name).map_err(|e| format!("{file}: {e}"))
}

/// 静态识别版本整数声明；只消费声明自身，避免将注释或字符串当作版本。
fn declaration_number(source: &str, name: &str) -> Option<u64> {
    let bytes = source.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        skip_trivia(bytes, &mut cursor).ok()?;
        if cursor >= bytes.len() {
            break;
        }
        if matches!(bytes[cursor], b'\'' | b'"' | b'`') {
            skip_string(bytes, &mut cursor).ok()?;
            continue;
        }
        if let Some(token) = identifier(bytes, &mut cursor) {
            if !matches!(token, "const" | "let" | "var") {
                continue;
            }
            skip_trivia(bytes, &mut cursor).ok()?;
            if identifier(bytes, &mut cursor) != Some(name) {
                continue;
            }
            skip_trivia(bytes, &mut cursor).ok()?;
            if bytes.get(cursor) != Some(&b'=') {
                continue;
            }
            cursor += 1;
            skip_trivia(bytes, &mut cursor).ok()?;
            let start = cursor;
            while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                cursor += 1;
            }
            let value = source[start..cursor].parse().ok()?;
            skip_trivia(bytes, &mut cursor).ok()?;
            return matches!(bytes.get(cursor), None | Some(b';')).then_some(value);
        }
        cursor += 1;
    }
    None
}

/// 只解析声明的数据字面量；后续自定义 JavaScript 保留给浏览器执行。
pub(crate) fn parse_declaration(source: &str, name: &str) -> Result<Value, String> {
    let bytes = source.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        skip_trivia(bytes, &mut cursor)?;
        if cursor >= bytes.len() {
            break;
        }
        if matches!(bytes[cursor], b'\'' | b'"' | b'`') {
            skip_string(bytes, &mut cursor)?;
            continue;
        }
        if let Some(token) = identifier(bytes, &mut cursor) {
            if !matches!(token, "const" | "let" | "var") {
                continue;
            }
            skip_trivia(bytes, &mut cursor)?;
            if identifier(bytes, &mut cursor) != Some(name) {
                continue;
            }
            skip_trivia(bytes, &mut cursor)?;
            if bytes.get(cursor) != Some(&b'=') {
                continue;
            }
            cursor += 1;
            skip_trivia(bytes, &mut cursor)?;
            let start = cursor;
            if !matches!(bytes.get(cursor), Some(b'{') | Some(b'[')) {
                return Err(format!("{name}: 需要 JSON5 对象或数组字面量"));
            }
            let mut stack = Vec::new();
            loop {
                skip_trivia(bytes, &mut cursor)?;
                let Some(&byte) = bytes.get(cursor) else {
                    return Err(format!("{name}: JSON5 字面量未闭合（起始字节 {start}）"));
                };
                match byte {
                    b'\'' | b'"' | b'`' => {
                        skip_string(bytes, &mut cursor)?;
                        continue;
                    }
                    b'{' => stack.push(b'}'),
                    b'[' => stack.push(b']'),
                    b'}' | b']' => {
                        if stack.pop() != Some(byte) {
                            return Err(format!("{name}: JSON5 括号不匹配，字节 {cursor}"));
                        }
                        if stack.is_empty() {
                            // 补齐前导换行，让 JSON5 的行号对应原资源文件。
                            let prefix = source[..start]
                                .chars()
                                .map(|c| if c == '\n' { '\n' } else { ' ' })
                                .collect::<String>();
                            return json5::from_str(&format!(
                                "{prefix}{}",
                                &source[start..=cursor]
                            ))
                            .map_err(|e| format!("{name}: {e}"));
                        }
                    }
                    _ => {}
                }
                cursor += 1;
            }
        }
        cursor += 1;
    }
    Err(format!("未找到 {name} 数据声明"))
}

fn identifier<'a>(bytes: &'a [u8], cursor: &mut usize) -> Option<&'a str> {
    let start = *cursor;
    if !bytes
        .get(start)
        .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'_' | b'$'))
    {
        return None;
    }
    *cursor += 1;
    while bytes
        .get(*cursor)
        .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'$'))
    {
        *cursor += 1;
    }
    std::str::from_utf8(&bytes[start..*cursor]).ok()
}

fn skip_string(bytes: &[u8], cursor: &mut usize) -> Result<(), String> {
    let quote = bytes[*cursor];
    *cursor += 1;
    while let Some(&byte) = bytes.get(*cursor) {
        *cursor += 1;
        if byte == quote {
            return Ok(());
        }
        if byte == b'\\' {
            *cursor += 1;
        }
    }
    Err("JSON5 字符串未闭合".into())
}

fn skip_trivia(bytes: &[u8], cursor: &mut usize) -> Result<(), String> {
    loop {
        while bytes.get(*cursor).is_some_and(u8::is_ascii_whitespace) {
            *cursor += 1;
        }
        match bytes.get(*cursor..*cursor + 2) {
            Some(b"//") => {
                *cursor += 2;
                while bytes.get(*cursor).is_some_and(|b| *b != b'\n') {
                    *cursor += 1;
                }
            }
            Some(b"/*") => {
                *cursor += 2;
                while bytes.get(*cursor..*cursor + 2) != Some(b"*/") {
                    if *cursor >= bytes.len() {
                        return Err("JSON5 注释未闭合".into());
                    }
                    *cursor += 1;
                }
                *cursor += 2;
            }
            _ => return Ok(()),
        }
    }
}
