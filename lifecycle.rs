//! 资源任务、配置插件生命周期和快照管理。
use crate::{
    Context, PLUGIN_ID,
    abi::{boundary, bytes, context},
};
use serde_json::Value;
use std::ffi::c_void;
use vchat_plugin_api::{abi::*, proto::StateSnapshot};

pub(crate) unsafe extern "C" fn create(
    host: *const HostApi,
    config: StringView,
    out: *mut PluginContext,
) -> i32 {
    boundary(|| {
        let data = config.data;
        let len = match usize::try_from(config.len) {
            Ok(value) => value,
            Err(_) => return STATUS_ERROR,
        };
        if host.is_null() || out.is_null() {
            return STATUS_ERROR;
        }
        // SAFETY: checked above; the host struct is copied before returning.
        unsafe {
            *out = std::ptr::null_mut();
        };
        let host = match unsafe { vchat_plugin_api::sdk::read_host_api(host) } {
            Ok(host) => host,
            Err(_) => return STATUS_ERROR,
        };
        // SAFETY: `out` is supplied by the host and points to writable plugin context storage.
        let Some(data) = (unsafe { bytes(data, len) }) else {
            return STATUS_ERROR;
        };
        let Ok(config) = serde_json::from_slice::<Value>(data) else {
            return STATUS_ERROR;
        };
        if !config.is_object() {
            return STATUS_ERROR;
        }
        let paths = match unsafe { vchat_plugin_api::runtime::runtime_paths(&host) } {
            Ok(paths) => paths,
            Err(error) => {
                eprintln!("Echo-Live 初始化失败: {error}");
                return STATUS_ERROR;
            }
        };
        let schema = crate::management::read_schema(&paths.resources_root, &config);
        let ctx = Context {
            host,
            resources: paths.resources_root,
            assets: paths.assets_root,
            jobs: Default::default(),
            resource_task: serde_json::Value::Null,
            config,
            schema,
        };
        if let Some(error) = ctx.schema["error"].as_str() {
            ctx.log(error);
        }
        unsafe { *out = Box::into_raw(Box::new(ctx)).cast() };
        STATUS_OK
    })
}

pub(crate) unsafe extern "C" fn start(ptr: PluginContext) -> i32 {
    boundary(|| {
        let Some(ctx) = (unsafe { context(ptr) }) else {
            return STATUS_ERROR;
        };
        ctx.log("Echo-Live 资源与接口插件启动");
        let payload = serde_json::json!({
            "version": 1,
            "routes": [
                {"kind":"http","method":"GET","path":"/Echo-Live/*","operation":"frontend.http"},
                {"kind":"http","method":"GET","path":"/echolive-media/*","operation":"frontend.http"},
                {"kind":"websocket","method":"GET","path":"/ws","operation":"frontend.ws"}
            ]
        });
        let payload_json = serde_json::to_vec(&payload).unwrap();
        let request = vchat_plugin_api::proto::HostRequest {
            service: "web.route.register".into(),
            payload_json,
            ..Default::default()
        };
        let status = unsafe {
            vchat_plugin_api::sdk::call_host(&ctx.host, &request)
                .map(|_| STATUS_OK)
                .unwrap_or_else(|error| {
                    eprintln!("插件宿主调用失败: {error}");
                    STATUS_ERROR
                })
        };
        if status != STATUS_OK {
            ctx.log("Echo-Live 路由注册失败");
            return status;
        }
        crate::management::register_ui(ctx, &ctx.config, &ctx.schema)
    })
}

pub(crate) unsafe extern "C" fn quiesce(ptr: PluginContext) -> i32 {
    boundary(|| {
        let Some(ctx) = (unsafe { context(ptr) }) else {
            return STATUS_ERROR;
        };
        ctx.log("Echo-Live 配置插件静止");
        ctx.jobs.shutdown();
        if let Ok(state) = ctx.jobs.cancel(None) {
            ctx.resource_task = state;
        }
        STATUS_OK
    })
}

pub(crate) unsafe extern "C" fn destroy(ptr: PluginContext) -> i32 {
    boundary(|| {
        if ptr.is_null() {
            return STATUS_OK;
        }
        // SAFETY: the context is uniquely owned by the plugin instance at destroy time.
        unsafe { drop(Box::from_raw(ptr.cast::<Context>())) };
        STATUS_OK
    })
}

pub(crate) unsafe extern "C" fn save_state(
    ptr: PluginContext,
    write: ReplySink,
    out: *mut c_void,
) -> i32 {
    boundary(|| {
        if ptr.is_null() {
            return STATUS_ERROR;
        }
        let snapshot = StateSnapshot {
            plugin_id: PLUGIN_ID.into(),
            format_version: 1,
            data: Vec::new(),
        };
        // SAFETY: 宿主复制回调在本次 ABI 调用期间有效。
        unsafe { vchat_plugin_api::sdk::write_snapshot(&snapshot, write, out) }
    })
}

pub(crate) unsafe extern "C" fn restore_state(
    ptr: PluginContext,
    snapshot: *const vchat_plugin_api::abi::StateSnapshot,
) -> i32 {
    boundary(|| {
        if ptr.is_null() {
            return STATUS_ERROR;
        }
        // 无业务状态仍校验快照归属和格式，支持管理器安全热更新。
        let Ok(snapshot) = (unsafe { vchat_plugin_api::sdk::read_snapshot(snapshot) }) else {
            return STATUS_ERROR;
        };
        if snapshot.plugin_id == PLUGIN_ID
            && snapshot.format_version == 1
            && snapshot.data.is_empty()
        {
            STATUS_OK
        } else {
            STATUS_ERROR
        }
    })
}
