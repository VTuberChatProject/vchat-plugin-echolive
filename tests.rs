//! C ABI 的空指针、协议错误和无状态快照验证。
use crate::*;
use prost::Message;
use std::ffi::c_void;
use vchat_plugin_api::proto::{PluginRequest, PluginResponse};

unsafe extern "C" fn host_invoke(
    _: HostContext,
    _: *const u8,
    _: usize,
    _: WriteCallback,
    _: *mut c_void,
) -> i32 {
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
    assert_eq!(
        unsafe { (API.create)(&host, std::ptr::null(), 0, &mut context) },
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
