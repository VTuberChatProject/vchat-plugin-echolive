//! 插件 C ABI 的请求边界、协议校验和响应复制。
use crate::{Context, HTTP_OPERATION, MAX_PAYLOAD, OPERATION, PROTOCOL_VERSION};
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
        if request.protocol_version != PROTOCOL_VERSION
            || !(matches!(
                request.operation.as_str(),
                OPERATION | HTTP_OPERATION | "config.changed"
            ) || request.operation.starts_with("frontend.ws."))
        {
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
        if request.operation == "config.changed" {
            return match crate::management::change_config(ctx, payload) {
                Ok(()) => {
                    let response = proto::PluginResponse {
                        payload_json: br#"{"version":1,"accepted":true}"#.to_vec(),
                        ..Default::default()
                    }
                    .encode_to_vec();
                    unsafe { write(out, response.as_ptr(), response.len()) }
                }
                Err(error) => write_error(ctx, write, out, "invalid_config", &error),
            };
        }
        if payload["version"].as_u64() != Some(1) {
            return write_error(
                ctx,
                write,
                out,
                "unsupported_version",
                "配置插件请求版本不兼容",
            );
        }
        if request.operation.starts_with("frontend.ws.") {
            return write_empty_response(write, out);
        }
        if request.operation == HTTP_OPERATION {
            return write_http_response(ctx, write, out, &payload);
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
        let script = crate::management::generate(ctx, source);
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

fn write_empty_response(write: WriteCallback, out: *mut c_void) -> i32 {
    let encoded = proto::PluginResponse::default().encode_to_vec();
    unsafe { write(out, encoded.as_ptr(), encoded.len()) }
}

fn write_http_response(
    ctx: &Context,
    write: WriteCallback,
    out: *mut c_void,
    payload: &serde_json::Value,
) -> i32 {
    if payload["version"].as_u64() != Some(1) || payload["method"].as_str() != Some("GET") {
        return write_error(ctx, write, out, "invalid_request", "Echo-Live 资源请求无效");
    }
    let Some(path) = payload["path"].as_str() else {
        return write_error(
            ctx,
            write,
            out,
            "invalid_path",
            "Echo-Live 资源请求缺少路径",
        );
    };
    if path == crate::output::BRIDGE_PATH {
        let response = proto::PluginResponse {
            status: 200,
            content_type: "text/javascript; charset=utf-8".into(),
            no_store: true,
            body: crate::output::BRIDGE.as_bytes().to_vec(),
            ..Default::default()
        }
        .encode_to_vec();
        return unsafe { write(out, response.as_ptr(), response.len()) };
    }
    let (resource_root, relative, plugin_media) =
        if let Some(relative) = path.strip_prefix("/Echo-Live/") {
            (ctx.resources.join("Echo-Live"), relative, false)
        } else if let Some(relative) = path.strip_prefix("/echolive-media/") {
            (ctx.resources.join("media"), relative, true)
        } else {
            return write_error(ctx, write, out, "not_found", "Echo-Live 插件未注册该资源");
        };
    if relative.is_empty()
        || relative.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.starts_with('.')
                || part.contains(['\\', ':'])
        })
    {
        return write_error(ctx, write, out, "not_found", "Echo-Live 资源路径无效");
    }
    let Ok(real_root) = std::fs::canonicalize(resource_root) else {
        return write_error(ctx, write, out, "not_found", "Echo-Live 资源目录不存在");
    };
    let candidate = real_root.join(relative);
    let Ok(real) = std::fs::canonicalize(&candidate) else {
        return write_error(ctx, write, out, "not_found", "Echo-Live 资源不存在");
    };
    if !real.starts_with(&real_root) {
        return write_error(ctx, write, out, "forbidden", "Echo-Live 资源路径越界");
    }
    let canonical = real
        .strip_prefix(&real_root)
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"));
    if canonical.as_deref() != Some(relative) || !real.is_file() {
        return write_error(ctx, write, out, "not_found", "Echo-Live 资源不存在");
    }
    let mut body = match std::fs::read(&real) {
        Ok(body) => body,
        Err(error) => {
            ctx.log(&format!("Echo-Live 资源读取失败: {error}"));
            return write_error(ctx, write, out, "read_failed", "Echo-Live 资源读取失败");
        }
    };
    let mut content_type = mime_guess::from_path(&real)
        .first_or_octet_stream()
        .to_string();
    let mut no_store = false;
    if !plugin_media && relative == "config.js" {
        let source = match String::from_utf8(body) {
            Ok(source) => source,
            Err(_) => {
                return write_error(
                    ctx,
                    write,
                    out,
                    "invalid_encoding",
                    "Echo-Live 配置编码无效",
                );
            }
        };
        body = crate::management::generate(ctx, &source).into_bytes();
        content_type = "text/javascript; charset=utf-8".into();
        no_store = true;
    }
    if !plugin_media && relative == "live.html" {
        let source = match String::from_utf8(body) {
            Ok(source) => source,
            Err(_) => {
                return write_error(
                    ctx,
                    write,
                    out,
                    "invalid_encoding",
                    "Echo-Live 页面编码无效",
                );
            }
        };
        let Some(html) = crate::output::inject_html(&source) else {
            return write_error(
                ctx,
                write,
                out,
                "invalid_html",
                "Echo-Live 页面缺少兼容脚本注入入口",
            );
        };
        body = html.into_bytes();
        no_store = true;
    }
    let response = proto::PluginResponse {
        status: 200,
        content_type,
        no_store,
        body,
        ..Default::default()
    };
    let encoded = response.encode_to_vec();
    if encoded.len() > MAX_PAYLOAD {
        return write_error(
            ctx,
            write,
            out,
            "response_too_large",
            "Echo-Live 资源响应过大",
        );
    }
    unsafe { write(out, encoded.as_ptr(), encoded.len()) }
}

fn write_error(
    ctx: &Context,
    write: WriteCallback,
    out: *mut c_void,
    code: &str,
    message: &str,
) -> i32 {
    ctx.log(message);
    let status = match code {
        "forbidden" => 403,
        "invalid_request" | "invalid_path" | "invalid_encoding" | "invalid_config" => 400,
        "read_failed" | "response_too_large" => 500,
        _ => 404,
    };
    let response = proto::PluginResponse {
        status,
        content_type: "text/plain; charset=utf-8".into(),
        body: message.as_bytes().to_vec(),
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
