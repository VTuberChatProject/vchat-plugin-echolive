//! 插件 C ABI 的请求边界、协议校验和响应复制。
use crate::{Context, MAX_PAYLOAD, OPERATION, PROTOCOL_VERSION, generate_config};
use prost::Message;
use std::{ffi::c_void, panic::AssertUnwindSafe};
use vchat_plugin_api::{abi::*, proto};

pub(crate) fn boundary(f: impl FnOnce() -> i32) -> i32 {
    std::panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or(STATUS_ERROR)
}

pub(crate) unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if len > MAX_PAYLOAD || (len != 0 && ptr.is_null()) {
        return None;
    }
    if len == 0 {
        Some(&[])
    } else {
        // SAFETY: The ABI caller owns the request buffer for the duration of the call.
        Some(unsafe { std::slice::from_raw_parts(ptr, len) })
    }
}

pub(crate) unsafe fn context<'a>(ptr: PluginContext) -> Option<&'a mut Context> {
    if ptr.is_null() {
        None
    } else {
        // SAFETY: `ptr` was allocated by `create` and remains valid until `destroy`.
        Some(unsafe { &mut *ptr.cast::<Context>() })
    }
}

pub(crate) unsafe extern "C" fn invoke(
    ptr: PluginContext,
    data: *const u8,
    len: usize,
    write: WriteCallback,
    out: *mut c_void,
) -> i32 {
    boundary(|| {
        // 调用期间由管理器串行持有上下文，ABI 边界拒绝空上下文。
        let Some(ctx) = (unsafe { context(ptr) }) else {
            return STATUS_ERROR;
        };
        let Some(data) = (unsafe { bytes(data, len) }) else {
            return STATUS_ERROR;
        };
        let Ok(request) = proto::PluginRequest::decode(data) else {
            ctx.log("配置插件 Protobuf 请求无效");
            return STATUS_ERROR;
        };
        if request.protocol_version != PROTOCOL_VERSION || request.operation != OPERATION {
            return write_error(
                ctx,
                write,
                out,
                "unsupported_operation",
                "不支持的配置插件操作",
            );
        }
        let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&request.payload_json) else {
            return write_error(
                ctx,
                write,
                out,
                "invalid_payload",
                "配置插件请求不是有效 JSON",
            );
        };
        if payload["version"].as_u64() != Some(1) {
            return write_error(
                ctx,
                write,
                out,
                "unsupported_version",
                "配置插件请求版本不兼容",
            );
        }
        let Some(source) = payload["source"].as_str() else {
            return write_error(
                ctx,
                write,
                out,
                "invalid_source",
                "配置插件请求缺少 source 脚本",
            );
        };
        let script = generate_config(source);
        let response = proto::PluginResponse {
            payload_json: serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "script": script,
            }))
            .expect("配置插件响应序列化不会失败"),
            ..Default::default()
        };
        let encoded = response.encode_to_vec();
        if encoded.len() > MAX_PAYLOAD {
            ctx.log("配置插件响应超出宿主容量限制");
            return STATUS_ERROR;
        }
        ctx.log("Echo-Live 配置注入完成");
        // SAFETY: callback and output context are owned by the host for this invocation.
        unsafe { write(out, encoded.as_ptr(), encoded.len()) }
    })
}

fn write_error(
    ctx: &Context,
    write: WriteCallback,
    out: *mut c_void,
    code: &str,
    message: &str,
) -> i32 {
    ctx.log(message);
    let response = proto::PluginResponse {
        error: Some(proto::PluginError {
            code: code.into(),
            message: message.into(),
        }),
        ..Default::default()
    };
    let encoded = response.encode_to_vec();
    // SAFETY: callback and output context are owned by the host for this invocation.
    unsafe { write(out, encoded.as_ptr(), encoded.len()) }
}

impl Context {
    /// 使用现有宿主日志通道记录生命周期和转换结果，日志包含结果说明。
    pub(crate) fn log(&self, message: &str) {
        let request = proto::HostRequest {
            service: "log".into(),
            payload_json: serde_json::to_vec(&serde_json::json!({"message": message})).unwrap(),
            ..Default::default()
        }
        .encode_to_vec();
        // SAFETY: HostApi 回调和上下文由管理器持有，调用期间保持有效。
        unsafe {
            (self.host.invoke)(
                self.host.context,
                request.as_ptr(),
                request.len(),
                ignore_write,
                std::ptr::null_mut(),
            );
        }
    }
}

unsafe extern "C" fn ignore_write(_: *mut c_void, _: *const u8, _: usize) -> i32 {
    STATUS_OK
}
