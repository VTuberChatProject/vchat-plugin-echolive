//! 无后台任务的配置插件生命周期和快照管理。
use crate::{
    Context, PLUGIN_ID,
    abi::{boundary, bytes, context},
};
use prost::Message;
use std::ffi::c_void;
use vchat_plugin_api::{abi::*, proto::StateSnapshot};

pub(crate) unsafe extern "C" fn create(
    host: *const HostApi,
    _data: *const u8,
    _len: usize,
    out: *mut PluginContext,
) -> i32 {
    boundary(|| {
        if host.is_null() || out.is_null() {
            return STATUS_ERROR;
        }
        // SAFETY: checked above; the host struct is copied before returning.
        let host = unsafe { *host };
        if host.abi_version != ABI_VERSION
            || host.struct_size < std::mem::size_of::<HostApi>() as u32
        {
            return STATUS_ERROR;
        }
        // SAFETY: `out` is supplied by the host and points to writable plugin context storage.
        unsafe { *out = Box::into_raw(Box::new(Context { host })).cast() };
        STATUS_OK
    })
}

pub(crate) unsafe extern "C" fn start(ptr: PluginContext) -> i32 {
    boundary(|| {
        let Some(ctx) = (unsafe { context(ptr) }) else {
            return STATUS_ERROR;
        };
        ctx.log("Echo-Live 配置插件启动");
        STATUS_OK
    })
}

pub(crate) unsafe extern "C" fn quiesce(ptr: PluginContext) -> i32 {
    boundary(|| {
        let Some(ctx) = (unsafe { context(ptr) }) else {
            return STATUS_ERROR;
        };
        ctx.log("Echo-Live 配置插件静止");
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
    write: WriteCallback,
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
        }
        .encode_to_vec();
        // SAFETY: 宿主复制回调在本次 ABI 调用期间有效。
        unsafe { write(out, snapshot.as_ptr(), snapshot.len()) }
    })
}

pub(crate) unsafe extern "C" fn restore_state(
    ptr: PluginContext,
    data: *const u8,
    len: usize,
) -> i32 {
    boundary(|| {
        if ptr.is_null() {
            return STATUS_ERROR;
        }
        // 无业务状态仍校验快照归属和格式，支持管理器安全热更新。
        let Some(data) = (unsafe { bytes(data, len) }) else {
            return STATUS_ERROR;
        };
        let Ok(snapshot) = StateSnapshot::decode(data) else {
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
