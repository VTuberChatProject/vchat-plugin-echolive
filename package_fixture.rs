//! 动态插件测试夹具：插件包与用户选择的外部 EchoLive 目录独立管理。
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};
use vchat_internal_plugin::{InternalPluginManager, PluginManifest, package::pack_directory};

/// 构建依赖一次，多场景分别持有自己的插件运行目录。
pub fn worker() -> &'static Path {
    static WORKER: OnceLock<PathBuf> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
                .current_dir(&root)
                .args([
                    "build",
                    "-p",
                    "vchat-internal-plugin",
                    "--bin",
                    "vchat-plugin-worker",
                ])
                .status()
                .expect("构建插件测试进程");
            assert!(status.success());
            let target = std::env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join("target"));
            target.join("debug").join(format!(
                "vchat-plugin-worker{}",
                std::env::consts::EXE_SUFFIX
            ))
        })
        .as_path()
}

/// 按真实平台和打包入口构造测试清单。
pub fn manifest() -> PluginManifest {
    let mut value: serde_json::Value = serde_json::from_str(include_str!("manifest.json")).unwrap();
    value["platform"] = std::env::consts::OS.into();
    value["architecture"] = std::env::consts::ARCH.into();
    value["entry"] = format!(
        "bin/{}vchat_echolive_config_plugin{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    )
    .into();
    serde_json::from_value(value).unwrap()
}

pub fn package(library: &Path, _resources: Option<&Path>) -> PathBuf {
    let staging = tempfile::tempdir().unwrap();
    let manifest = manifest();
    std::fs::create_dir_all(staging.path().join("bin")).unwrap();
    std::fs::copy(library, staging.path().join(&manifest.entry)).unwrap();
    std::fs::write(
        staging.path().join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    // 输出置于暂存树外，避免归档将自身包含进去。
    let archive = staging.path().with_extension("vcplug");
    pack_directory(staging.path(), &archive).unwrap();
    archive
}

/// 测试显式选择开发资源目录，与生产用户选择文件夹使用相同配置契约。
pub fn config(resources: Option<&Path>) -> serde_json::Value {
    let default = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Assets/Frontend/Echo-Live");
    let root = resources.unwrap_or(&default).canonicalize().unwrap();
    serde_json::json!({"echolive_config":{},"echolive_resource":{
        "id":root.to_string_lossy(),"path":root,"source":"folder",
        "data_version":17,"capabilities":{"live":true,"history":true,"bridge":true}
    }})
}

pub fn manager(cache: &Path) -> InternalPluginManager {
    let mut manager = InternalPluginManager::new(cache);
    manager.set_worker_executable(worker());
    manager
}
