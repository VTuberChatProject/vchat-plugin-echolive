//! 外部资源任务与配置档案的插件边界；候选设置交由宿主持久化事务确认。
use crate::Context;
use serde_json::{Value, json};
use std::path::PathBuf;

/// 每次 ABI 调用只处理任务快照，下载及解压留在独立后台线程。
pub(crate) fn invoke(ctx: &mut Context, operation: &str, payload: &Value) -> Result<Value, String> {
    // 在真正执行操作的插件代次中检查身份，覆盖宿主检查与进程锁之间的切换窗口。
    if payload["generation"].as_u64() != Some(ctx.host.generation) {
        return Err("EchoLive 资源请求代次已过期".into());
    }
    let mut state = match operation {
        "echolive.resources.prepare" => {
            let action = payload["action"].as_str().ok_or("缺少资源操作")?;
            let assets = ctx
                .assets
                .clone()
                .or_else(|| (action == "folder").then(PathBuf::new))
                .ok_or("当前宿主未提供 Assets 目录，请更新 VChat")?;
            ctx.jobs.start(
                assets,
                action.into(),
                payload["path"].as_str().map(PathBuf::from),
            )?
        }
        "echolive.resources.status" => ctx
            .jobs
            .status(payload["id"].as_str().ok_or("缺少任务标识")?)?,
        "echolive.resources.cancel" => ctx.jobs.cancel(payload["id"].as_str())?,
        _ => return Err("未知 EchoLive 资源操作".into()),
    };
    for message in ctx.jobs.drain_logs() {
        ctx.log(&message);
    }
    if state["busy"] == false
        && state["resource"].is_object()
        && state["error"].as_str().is_none_or(str::is_empty)
    {
        match candidate(&ctx.config, state["resource"].clone()) {
            Ok(candidate) => state["candidate"] = candidate,
            Err(error) => {
                state["stage"] = json!("failed");
                state["error"] = json!(error);
                ctx.log(&format!("准备资源配置失败: {error}"));
            }
        }
    }
    ctx.resource_task = state.clone();
    if let Some(state) = ctx.resource_task.as_object_mut() {
        state.remove("candidate");
    }
    if crate::management::register_ui(ctx, &ctx.config, &ctx.schema)
        != vchat_plugin_api::abi::STATUS_OK
    {
        return Err("EchoLive 资源状态发布失败".into());
    }
    Ok(state)
}

/// 档案以资源身份隔离；首次接入仅继承目标版本可接受的覆盖。
fn candidate(current: &Value, resource: Value) -> Result<Value, String> {
    let root = resource["path"].as_str().ok_or("资源结果缺少目录")?;
    let id = resource["id"].as_str().ok_or("资源结果缺少身份")?;
    let schema = crate::schema::load_schema(std::path::Path::new(root))?;
    let mut next = current.clone();
    let mut profiles = current
        .get("echolive_profiles")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let existing = current
        .get("echolive_config")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(previous) = current["echolive_resource"]["id"].as_str() {
        profiles[previous] = json!({"resource":current["echolive_resource"],"overrides":existing});
    }
    let overrides = profiles[id]
        .get("overrides")
        .cloned()
        .unwrap_or_else(|| crate::validation::compatible_overrides(&schema, &existing));
    next["echolive_resource"] = resource.clone();
    next["echolive_config"] = overrides.clone();
    profiles[id] = json!({"resource":resource,"overrides":overrides});
    next["echolive_profiles"] = profiles;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两份资源分别保存自己的覆盖，首次接入按目标定义继承兼容值。
    #[test]
    fn profiles_restore_each_directory_and_inherit_only_compatible_fields() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../Assets/Frontend/Echo-Live")
            .canonicalize()
            .unwrap();
        let a = json!({"id":"a","path":root,"source":"folder"});
        let b = json!({"id":"b","path":root,"source":"folder"});
        let mut first = candidate(
            &json!({"echolive_config":{"echo.print_speed":47,"custom.old":true}}),
            a.clone(),
        )
        .unwrap();
        assert_eq!(first["echolive_config"], json!({"echo.print_speed":47}));
        first["echolive_config"]["global.theme"] = json!("glass");
        let mut second = candidate(&first, b.clone()).unwrap();
        assert_eq!(second["echolive_config"]["global.theme"], "glass");
        second["echolive_config"]["echo.print_speed"] = json!(80);
        let restored = candidate(&second, a).unwrap();
        assert_eq!(restored["echolive_config"]["echo.print_speed"], 47);
        assert_eq!(
            restored["echolive_profiles"]["b"]["overrides"]["echo.print_speed"],
            80
        );
        let restored_b = candidate(&restored, b).unwrap();
        assert_eq!(restored_b["echolive_config"]["echo.print_speed"], 80);
    }
}
