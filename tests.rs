//! C ABI 的空指针、协议错误和无状态快照验证。
use crate::*;
use prost::Message;
use std::ffi::c_void;
use vchat_plugin_api::proto::{PluginRequest, PluginResponse};

unsafe extern "C" fn host_invoke(
    _: HostContext,
    data: *const u8,
    len: usize,
    write: WriteCallback,
    out: *mut c_void,
) -> i32 {
    let request = unsafe { std::slice::from_raw_parts(data, len) };
    let request = vchat_plugin_api::proto::HostRequest::decode(request).unwrap();
    if request.service == "runtime.paths" {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Assets/Frontend");
        let response = serde_json::to_vec(&vchat_plugin_api::runtime::RuntimePaths {
            package_root: root.clone(),
            resources_root: root.clone(),
            data_root: root,
            assets_root: None,
        })
        .unwrap();
        return unsafe { write(out, response.as_ptr(), response.len()) };
    }
    STATUS_OK
}

unsafe extern "C" fn write(out: *mut c_void, data: *const u8, len: usize) -> i32 {
    let output = unsafe { &mut *out.cast::<Vec<u8>>() };
    output.extend_from_slice(unsafe { std::slice::from_raw_parts(data, len) });
    STATUS_OK
}

fn instance() -> PluginContext {
    let host = HostApi {
        abi_version: ABI_VERSION,
        struct_size: std::mem::size_of::<HostApi>() as u32,
        generation: 1,
        context: std::ptr::null_mut(),
        invoke: host_invoke,
    };
    let mut context = std::ptr::null_mut();
    let config = br#"{}"#;
    assert_eq!(
        unsafe { (API.create)(&host, config.as_ptr(), config.len(), &mut context) },
        STATUS_OK
    );
    assert_eq!(unsafe { (API.start)(context) }, STATUS_OK);
    context
}

#[test]
fn malformed_abi_buffers_and_null_lifecycle_are_rejected() {
    let context = instance();
    let mut output = Vec::<u8>::new();
    assert_eq!(
        unsafe {
            (API.invoke)(
                context,
                std::ptr::null(),
                1,
                write,
                (&mut output as *mut Vec<u8>).cast(),
            )
        },
        STATUS_ERROR
    );
    assert_eq!(unsafe { (API.start)(std::ptr::null_mut()) }, STATUS_ERROR);
    assert_eq!(unsafe { (API.destroy)(context) }, STATUS_OK);
}

#[test]
fn operation_and_payload_errors_return_structured_plugin_errors() {
    let context = instance();
    for (operation, payload, expected) in [
        ("unknown", serde_json::json!({}), "unsupported_operation"),
        (
            OPERATION,
            serde_json::json!({"version":2,"source":"x"}),
            "unsupported_version",
        ),
        (
            OPERATION,
            serde_json::json!({"version":1}),
            "invalid_source",
        ),
    ] {
        let request = PluginRequest {
            protocol_version: 1,
            operation: operation.into(),
            payload_json: serde_json::to_vec(&payload).unwrap(),
            ..Default::default()
        }
        .encode_to_vec();
        let mut output = Vec::new();
        assert_eq!(
            unsafe {
                (API.invoke)(
                    context,
                    request.as_ptr(),
                    request.len(),
                    write,
                    (&mut output as *mut Vec<u8>).cast(),
                )
            },
            STATUS_OK
        );
        let response = PluginResponse::decode(output.as_slice()).unwrap();
        assert_eq!(response.error.unwrap().code, expected);
    }
    assert_eq!(unsafe { (API.destroy)(context) }, STATUS_OK);
}

#[test]
fn stateless_snapshot_has_plugin_identity_and_round_trips() {
    let context = instance();
    let mut output = Vec::new();
    assert_eq!(
        unsafe { (API.save_state)(context, write, (&mut output as *mut Vec<u8>).cast()) },
        STATUS_OK
    );
    assert_eq!(
        unsafe { (API.restore_state)(context, output.as_ptr(), output.len()) },
        STATUS_OK
    );
    assert_eq!(
        unsafe { (API.restore_state)(context, b"invalid".as_ptr(), 7) },
        STATUS_ERROR
    );
    assert_eq!(unsafe { (API.quiesce)(context) }, STATUS_OK);
    assert_eq!(unsafe { (API.destroy)(context) }, STATUS_OK);
}

/// 表单发布失败时业务配置与配置定义保持原值，允许用户保留草稿重试。
#[test]
fn rejected_ui_registration_keeps_previous_context() {
    unsafe extern "C" fn reject(
        _: HostContext,
        _: *const u8,
        _: usize,
        _: WriteCallback,
        _: *mut c_void,
    ) -> i32 {
        STATUS_ERROR
    }
    let ptr = instance();
    let ctx = unsafe { abi::context(ptr).unwrap() };
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Assets/Frontend/Echo-Live");
    ctx.config =
        serde_json::json!({"echolive_config":{},"echolive_resource":{"id":"test","path":root}});
    ctx.schema = schema::load_schema(&root).unwrap();
    let previous = ctx.config.clone();
    let previous_schema = ctx.schema.clone();
    let mut candidate = previous.clone();
    candidate["echolive_config"] = serde_json::json!({"echo.print_speed":48});
    ctx.host.invoke = reject;
    assert!(management::change_config(ctx, candidate).is_err());
    assert_eq!(ctx.config, previous);
    assert_eq!(ctx.schema, previous_schema);
    assert_eq!(unsafe { (API.destroy)(ptr) }, STATUS_OK);
}
