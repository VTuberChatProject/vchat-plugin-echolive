//! 为动态插件测试构建真实 ZIP 包与独立 worker，资源全部来自包内。
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

/// 复制资源时保持目录结构，夹具不借助运行时外部路径覆盖。
fn copy_tree(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            if entry.file_name() != ".git" {
                copy_tree(&entry.path(), &target.join(entry.file_name()));
            }
        } else {
            if entry.file_name() == ".git" {
                continue;
            }
            std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

pub fn package(library: &Path, resources: Option<&Path>) -> PathBuf {
    let staging = tempfile::tempdir().unwrap();
    let manifest = manifest();
    std::fs::create_dir_all(staging.path().join("bin")).unwrap();
    std::fs::copy(library, staging.path().join(&manifest.entry)).unwrap();
    std::fs::write(
        staging.path().join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let default_resources =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Assets/Frontend/Echo-Live");
    copy_tree(
        resources.unwrap_or(&default_resources),
        &staging.path().join("resources/Echo-Live"),
    );
    // 输出置于暂存树外，避免归档将自身包含进去。
    let archive = staging.path().with_extension("vcplug");
    pack_directory(staging.path(), &archive).unwrap();
    archive
}

pub fn manager(cache: &Path) -> InternalPluginManager {
    let mut manager = InternalPluginManager::new(cache);
    manager.set_worker_executable(worker());
    manager
}
