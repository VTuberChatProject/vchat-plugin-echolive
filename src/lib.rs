//! Echo-Live 资源、配置适配和 WebSocket 路由内置插件。
#[path = "adapter/abi.rs"]
mod abi;
#[path = "model/config.rs"]
mod config;
#[path = "application/lifecycle.rs"]
mod lifecycle;
#[path = "application/management.rs"]
mod management;
#[path = "adapter/output.rs"]
mod output;
#[path = "adapter/output_cards.rs"]
mod output_cards;
#[path = "infrastructure/resource_archive.rs"]
mod resource_archive;
#[path = "infrastructure/resource_download.rs"]
mod resource_download;
#[path = "application/resource_jobs.rs"]
mod resource_jobs;
#[path = "application/resource_management.rs"]
mod resource_management;
#[path = "infrastructure/resource_manifest.rs"]
mod resource_manifest;
#[path = "infrastructure/resources.rs"]
mod resources;
#[path = "model/schema.rs"]
mod schema;
#[path = "model/validation.rs"]
mod validation;
#[path = "model/versions.rs"]
mod versions;

use abi::invoke;
pub use config::generate_config;
use lifecycle::*;
use vchat_plugin_api::abi::*;

pub const PLUGIN_ID: &str = "builtin.echolive";
pub const OPERATION: &str = "frontend.config.generate";
pub const HTTP_OPERATION: &str = "frontend.http";
pub const PROTOCOL_VERSION: u32 = 1;
const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

struct Context {
    host: HostApi,
    resources: std::path::PathBuf,
    assets: Option<std::path::PathBuf>,
    jobs: resource_jobs::Jobs,
    resource_task: serde_json::Value,
    config: serde_json::Value,
    schema: serde_json::Value,
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
#[path = "../tests/unit/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/resources/resource_history_tests.rs"]
mod resource_history_tests;
#[cfg(test)]
#[path = "../tests/resources/resource_tests.rs"]
mod resource_tests;
#[cfg(test)]
#[path = "../tests/unit/schema_tests.rs"]
mod schema_tests;
#[cfg(test)]
#[path = "../tests/unit/versions_tests.rs"]
mod versions_tests;

#[cfg(test)]
#[path = "../tests/unit/test_resources.rs"]
mod test_resources;
