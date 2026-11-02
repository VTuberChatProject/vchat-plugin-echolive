//! Echo-Live 用户覆盖值的类型、数值范围和受控字段校验。
use serde_json::Value;

/// 保存前完整检查覆盖映射，避免部分有效字段提前进入运行上下文。
pub(crate) fn validate_overrides(schema: &Value, overrides: &Value) -> Result<(), String> {
    validate_overrides_preserving(schema, overrides, &serde_json::json!({}))
}

/// 历史档案允许原样保留失效值；新增和修改的字段仍须符合当前版本定义。
pub(crate) fn validate_overrides_preserving(
    schema: &Value,
    overrides: &Value,
    previous: &Value,
) -> Result<(), String> {
    let values = overrides
        .as_object()
        .ok_or("EchoLive 配置必须为路径到 JSON 值的对象")?;
    let fields = schema["fields"]
        .as_array()
        .ok_or("EchoLive 配置定义缺少 fields")?;
    for (path, value) in values {
        if !crate::schema::safe_path(path) {
            return Err(format!("{path}: 配置路径无效"));
        }
        let field = fields.iter().find(|f| f["path"].as_str() == Some(path));
        let inactive = schema["read_only"] == true
            || field.is_none_or(|field| {
                field["read_only"] == true || validate_value(field, value).is_err()
            });
        if inactive && previous.get(path) == Some(value) {
            continue;
        }
        if schema["read_only"] == true {
            return Err(format!("{path}: 当前配置版本为只读"));
        }
        let field = field.ok_or_else(|| format!("{path}: 配置项已删除或不存在"))?;
        if field["read_only"] == true {
            return Err(format!("{path}: 连接配置由 VChat 管理"));
        }
        validate_value(field, value)?;
    }
    Ok(())
}

/// 首次接入只继承当前版本可编辑且类型兼容的值；兼容官方 from 字段更名。
pub(crate) fn compatible_overrides(schema: &Value, overrides: &Value) -> Value {
    let mut result = serde_json::Map::new();
    if schema["read_only"] == true {
        return Value::Object(result);
    }
    for field in schema["fields"].as_array().into_iter().flatten() {
        let Some(path) = field["path"]
            .as_str()
            .filter(|p| crate::schema::safe_path(p))
        else {
            continue;
        };
        if field["read_only"] == true {
            continue;
        }
        let value = overrides
            .get(path)
            .or_else(|| field["from"].as_str().and_then(|from| overrides.get(from)));
        if let Some(value) = value.filter(|value| validate_value(field, value).is_ok()) {
            result.insert(path.to_owned(), value.clone());
        }
    }
    Value::Object(result)
}

/// 保留官方特殊类型：位布尔为 0/1，列表为 all 或字符串数组。
pub(crate) fn validate_value(field: &Value, value: &Value) -> Result<(), String> {
    let path = field["path"].as_str().unwrap_or("未知字段");
    let kind = field["kind"].as_str().unwrap_or("");
    let valid = match kind {
        "boolean" => value.is_boolean(),
        "boolean.bit" => value == 0 || value == 1,
        "string" | "string.multiline" => value.is_string(),
        "number" | "special.fontsize" => value.as_f64().is_some_and(f64::is_finite),
        "special.all_or_array_string" => {
            value == "all"
                || value
                    .as_array()
                    .is_some_and(|list| list.iter().all(Value::is_string))
        }
        _ => return Err(format!("{path}: 暂不支持的官方类型 {kind}")),
    };
    if !valid {
        return Err(format!("{path}: 需要 {kind} 类型的值"));
    }
    if let Some(number) = value.as_f64() {
        if let Some(minimum) = field["minimum"].as_f64()
            && number < minimum
        {
            return Err(format!("{path}: 最小值为 {minimum}"));
        }
        if let Some(maximum) = field["maximum"].as_f64()
            && number > maximum
        {
            return Err(format!("{path}: 最大值为 {maximum}"));
        }
        // step 记录官方输入增量，官方默认值也可能不对齐；范围构成保存约束。
        if kind == "special.fontsize" && number <= 0.0 {
            return Err(format!("{path}: 字体大小必须大于 0"));
        }
    }
    Ok(())
}
