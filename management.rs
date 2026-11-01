//! 配置表单声明、原子更新与资源脚本生成的插件业务边界。
use crate::Context;
use serde_json::{Value, json};
use std::path::Path;
use vchat_plugin_api::{abi::*, proto};

/// 解析失败作为表单错误展示，保持原有资源服务可用。
pub(crate) fn read_schema(_resources: &Path, config: &Value) -> Value {
    let Some(root) = resource_root(config) else {
        return json!({"version":1,"fields":[],"read_only":true,"error":"请选择 EchoLive 文件夹、下载或导入 ZIP"});
    };
    let result = crate::schema::load_schema(root).and_then(|schema| {
        if let Some(values) = config.get("echolive_config") {
            crate::validation::validate_overrides_preserving(&schema, values, values)?;
        }
        Ok(schema)
    });
    result.unwrap_or_else(|error| json!({"version":1,"fields":[],"error":error}))
}

/// EchoLive 仅从用户确认的外部资源目录加载。
pub(crate) fn resource_root(config: &Value) -> Option<&Path> {
    config["echolive_resource"]["path"].as_str().map(Path::new)
}

/// 表单值只携带用户覆盖，配置定义由独立协议字段传输。
pub(crate) fn register_ui(ctx: &Context, config: &Value, schema: &Value) -> i32 {
    let values = config
        .get("echolive_config")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let request = proto::HostRequest {
        service: "ui.register".into(),
        ui: Some(proto::UiContribution {
            output_cards: resource_root(config).map(|root| crate::output_cards::cards().into_iter()
                .filter(|card| root.join(if card.id == "history" { "history.html" } else { "live.html" }).is_file())
                .collect()).unwrap_or_default(),
            fields: vec![proto::UiField {
                id: "echolive_config".into(),
                label: "EchoLive 配置".into(),
                kind: "echolive_config".into(),
                value_json: values.to_string(),
                schema_json: schema.to_string(),
                ..Default::default()
            }, proto::UiField {
                id: "echolive_resource".into(),
                label: "EchoLive 资源".into(),
                kind: "echolive_resources".into(),
                value_json: json!({"resource":config.get("echolive_resource"),"task":ctx.resource_task,"assets_root":ctx.assets}).to_string(),
                ..Default::default()
            }, proto::UiField {
                id: "echolive_profiles".into(),
                label: "EchoLive 配置档案".into(),
                kind: "echolive_profiles".into(),
                value_json: config
                    .get("echolive_profiles")
                    .cloned()
                    .unwrap_or_else(|| json!({"version":1,"current_id":"legacy","profiles":[{"id":"legacy","name":"当前配置","values":values}]}))
                    .to_string(),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    // SAFETY: 宿主同步复制请求，回调上下文在当前调用内有效。
    let status = unsafe {
        vchat_plugin_api::sdk::call_host(&ctx.host, &request)
            .map(|_| STATUS_OK)
            .unwrap_or_else(|error| {
                eprintln!("插件宿主调用失败: {error}");
                STATUS_ERROR
            })
    };
    if status != STATUS_OK {
        ctx.log("EchoLive 配置表单注册失败");
    }
    if status == STATUS_OK {
        let outputs = resource_root(config).map(|root| ["live", "history"].into_iter()
            .filter(|card| root.join(format!("{card}.html")).is_file())
            .map(|card| json!({"card_id":card,"receipt_mode":if card == "history" {"accepted"} else {"completed"}})).collect::<Vec<_>>()).unwrap_or_default();
        let registration = proto::HostRequest {
            service: "frontend.register".into(),
            payload_json: json!({"version":2,"outputs":outputs})
                .to_string()
                .into_bytes(),
            ..Default::default()
        };
        if let Err(error) = unsafe { vchat_plugin_api::sdk::call_host(&ctx.host, &registration) } {
            ctx.log(&format!("EchoLive 输出卡片注册失败: {error}"));
            return STATUS_ERROR;
        }
    }
    status
}

/// 全量校验与 UI 注册成功后再替换上下文，失败保留上次有效状态。
pub(crate) fn change_config(ctx: &mut Context, candidate: Value) -> Result<(), String> {
    let schema = validate_config(ctx, &candidate)?;
    if register_ui(ctx, &candidate, &schema) != STATUS_OK {
        return Err("EchoLive 配置表单更新失败".into());
    }
    ctx.config = candidate;
    ctx.schema = schema;
    ctx.log("EchoLive 配置已更新，请刷新输出页面以应用配置");
    Ok(())
}

/// 资源激活前仅验证候选设置，HTTP 和输出继续读取上次已确认的资源。
pub(crate) fn validate_config(ctx: &Context, candidate: &Value) -> Result<Value, String> {
    if !candidate.is_object() {
        return Err("EchoLive 配置必须是对象".into());
    }
    let schema = if let Some(root) = resource_root(candidate) {
        crate::resources::validate(root)?;
        crate::schema::load_schema(root)?
    } else {
        // 首次接入持久化失败时，允许事务恢复尚未选择资源的原始状态。
        let values = candidate.get("echolive_config");
        if values.is_some_and(|value| !value.as_object().is_some_and(|map| map.is_empty())) {
            return Err("请先选择 EchoLive 资源目录".into());
        }
        read_schema(&ctx.resources, candidate)
    };
    let empty = json!({});
    let previous = previous_overrides(&ctx.config, candidate).unwrap_or(&empty);
    let overrides = candidate.get("echolive_config").unwrap_or(&empty);
    if previous
        .as_object()
        .is_some_and(|values| !values.is_empty())
    {
        crate::validation::validate_overrides_preserving(&schema, overrides, previous)?;
    } else {
        crate::validation::validate_overrides(&schema, overrides)?;
    }
    validate_user_profiles(
        candidate.get("echolive_profiles"),
        ctx.config.get("echolive_profiles"),
        &schema,
    )?;
    Ok(schema)
}

/// 验证 UI 档案结构与覆盖值，保留旧版按资源 ID 索引的内部档案格式。
fn validate_user_profiles(
    profiles: Option<&Value>,
    previous: Option<&Value>,
    schema: &Value,
) -> Result<(), String> {
    let Some(profiles) = profiles else {
        return Ok(());
    };
    if !profiles.is_object() {
        return Err("EchoLive 配置档案格式无效".into());
    }
    if profiles.get("version").is_none() {
        return Ok(());
    }
    if profiles["version"].as_u64() != Some(1) {
        return Err("EchoLive 配置档案版本不兼容".into());
    }
    let entries = profiles["profiles"]
        .as_array()
        .ok_or("EchoLive 配置档案列表无效")?;
    let current = profiles["current_id"]
        .as_str()
        .ok_or("EchoLive 当前配置档案无效")?;
    let mut ids = std::collections::HashSet::new();
    for profile in entries {
        let id = profile["id"]
            .as_str()
            .filter(|id| !id.trim().is_empty())
            .ok_or("EchoLive 配置档案标识无效")?;
        if !ids.insert(id) {
            return Err("EchoLive 配置档案标识重复".into());
        }
        if profile["name"]
            .as_str()
            .is_none_or(|name| name.trim().is_empty())
        {
            return Err("EchoLive 配置档案名称无效".into());
        }
        let values = profile.get("values").ok_or("EchoLive 配置档案缺少配置值")?;
        let previous_values = previous
            .and_then(|previous| previous["profiles"].as_array())
            .into_iter()
            .flatten()
            .find(|old| old["id"].as_str() == Some(id))
            .and_then(|old| old.get("values"))
            .unwrap_or(&Value::Null);
        crate::validation::validate_overrides_preserving(schema, values, previous_values)?;
    }
    if !ids.contains(current) {
        return Err("EchoLive 当前配置档案不存在".into());
    }
    Ok(())
}

/// 当前资源以最新已确认值为准；切换资源时优先使用目标档案的历史覆盖。
fn previous_overrides<'a>(current: &'a Value, candidate: &Value) -> Option<&'a Value> {
    let target = candidate["echolive_resource"]["id"].as_str();
    if target == current["echolive_resource"]["id"].as_str() {
        return current.get("echolive_config");
    }
    target
        .and_then(|id| {
            current["echolive_resource_profiles"]
                .get(id)
                .or_else(|| current["echolive_profiles"].get(id))
        })
        .and_then(|profile| profile.get("overrides"))
        .or_else(|| current.get("echolive_config"))
}

/// 脚本生成与 HTTP 路由使用同一个已验证配置快照。
pub(crate) fn generate(ctx: &Context, source: &str) -> String {
    let empty = json!({});
    let valid = ctx.schema.get("error").is_none();
    let overrides = if valid {
        ctx.config.get("echolive_config").unwrap_or(&empty)
    } else {
        &empty
    };
    crate::config::generate_config_with(source, valid.then_some(&ctx.schema), overrides)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 资源外部更新后，最近保存的失效值仍可原样保留；过时档案不能阻止再次保存。
    #[test]
    fn current_resource_validation_uses_latest_overrides_before_profile_snapshot() {
        let current = json!({"echolive_resource":{"id":"a"},
            "echolive_config":{"old.field":"最近保存"},
            "echolive_resource_profiles":{"a":{"overrides":{"old.field":"旧快照"}},
                "b":{"overrides":{"old.field":"另一目录"}}}});
        let schema = json!({"fields":[]});
        let previous = previous_overrides(&current, &current).unwrap();
        crate::validation::validate_overrides_preserving(
            &schema,
            &current["echolive_config"],
            previous,
        )
        .unwrap();
        let switched = json!({"echolive_resource":{"id":"b"}});
        assert_eq!(
            previous_overrides(&current, &switched).unwrap()["old.field"],
            "另一目录"
        );
    }

    #[test]
    fn profile_contract_requires_valid_selected_entry_and_field_values() {
        let schema = crate::schema::load_schema(&crate::test_resources::root()).unwrap();
        let valid = json!({"version":1,"current_id":"one","profiles":[
            {"id":"one","name":"直播","values":{"echo.print_speed":47}}
        ]});
        validate_user_profiles(Some(&valid), None, &schema).unwrap();
        let invalid = json!({"version":1,"current_id":"one","profiles":[
            {"id":"one","name":"直播","values":{"echo.print_speed":"快"}}
        ]});
        assert!(validate_user_profiles(Some(&invalid), None, &schema).is_err());
        let missing = json!({"version":1,"current_id":"missing","profiles":[]});
        assert!(validate_user_profiles(Some(&missing), None, &schema).is_err());
        let historical = json!({"version":1,"current_id":"one","profiles":[
            {"id":"one","name":"直播","values":{"retired.option":"kept"}}
        ]});
        validate_user_profiles(Some(&historical), Some(&historical), &schema).unwrap();
    }
}
