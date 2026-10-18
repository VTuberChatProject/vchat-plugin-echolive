//! Echo-Live 资源、配置适配和 WebSocket 路由内置插件。
mod abi;
mod config;
mod lifecycle;

use abi::invoke;
pub use config::generate_config;
use lifecycle::*;
use vchat_plugin_api::abi::*;

pub const PLUGIN_ID: &str = "builtin.echolive";
pub const OPERATION: &str = "frontend.config.generate";
pub const HTTP_OPERATION: &str = "frontend.http";
pub const PROTOCOL_VERSION: u32 = 1;
pub const MANIFEST_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/echolive.internal.json"));
const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

struct Context {
    host: HostApi,
    config: serde_json::Value,
}

static API: PluginApi = PluginApi {
    abi_version: ABI_VERSION,
    struct_size: std::mem::size_of::<PluginApi>() as u32,
    create,
    start,
    invoke,
    quiesce,
    resume: start,
    save_state,
    restore_state,
    destroy,
};

#[unsafe(no_mangle)]
pub extern "C" fn vchat_internal_plugin_get_api() -> *const PluginApi {
    &API
}

#[cfg(test)]
mod tests;
