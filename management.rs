//! 配置表单声明、原子更新与资源脚本生成的插件业务边界。
use crate::Context;
use prost::Message;
use serde_json::{Value, json};
use std::{ffi::c_void, path::Path};
use vchat_plugin_api::{abi::*, proto};

/// 解析失败作为表单错误展示，保持原有资源服务可用。
pub(crate) fn read_schema(config: &Value) -> Value {
    let root = Path::new(config["echo_root"].as_str().unwrap_or_default());
    let result = crate::schema::load_schema(root).and_then(|schema| {
        if let Some(values) = config.get("echolive_config") {
            crate::validation::validate_overrides(&schema, values)?;
        }
        Ok(schema)
    });
    result.unwrap_or_else(|error| json!({"version":1,"fields":[],"error":error}))
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
            fields: vec![proto::UiField {
                id: "echolive_config".into(),
                label: "EchoLive 配置".into(),
                kind: "echolive_config".into(),
                value_json: values.to_string(),
                schema_json: schema.to_string(),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    }
    .encode_to_vec();
    // SAFETY: 宿主同步复制请求，回调上下文在当前调用内有效。
    let status = unsafe {
        (ctx.host.invoke)(
            ctx.host.context,
            request.as_ptr(),
            request.len(),
            ignore_write,
            std::ptr::null_mut(),
        )
    };
    if status != STATUS_OK {
        ctx.log("EchoLive 配置表单注册失败");
    }
    status
}

/// 全量校验与 UI 注册成功后再替换上下文，失败保留上次有效状态。
pub(crate) fn change_config(ctx: &mut Context, mut candidate: Value) -> Result<(), String> {
    if !candidate.is_object() {
        return Err("EchoLive 配置必须是对象".into());
    }
    // 资源路径由启动环境决定，插件表单只更新配置覆盖值。
    for key in ["echo_root", "media_root"] {
        candidate[key] = ctx.config[key].clone();
    }
    let schema = crate::schema::load_schema(Path::new(
        candidate["echo_root"].as_str().unwrap_or_default(),
    ))?;
    let empty = json!({});
    crate::validation::validate_overrides(
        &schema,
        candidate.get("echolive_config").unwrap_or(&empty),
    )?;
    if register_ui(ctx, &candidate, &schema) != STATUS_OK {
        return Err("EchoLive 配置表单更新失败".into());
    }
    ctx.config = candidate;
    ctx.schema = schema;
    ctx.log("EchoLive 配置已更新，请刷新输出页面以应用配置");
    Ok(())
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

unsafe extern "C" fn ignore_write(_: *mut c_void, _: *const u8, _: usize) -> i32 {
    STATUS_OK
}
