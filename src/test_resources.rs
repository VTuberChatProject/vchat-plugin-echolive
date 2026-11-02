//! 从官方离线夹具生成配置资源，单元测试可独立运行。
use std::{path::PathBuf, sync::OnceLock};

/// 同一测试进程共用只读资源，临时目录在进程结束时释放。
pub(crate) fn root() -> PathBuf {
    static ROOT: OnceLock<tempfile::TempDir> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = tempfile::tempdir().expect("创建离线资源夹具");
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("version_fixture_17.json")).unwrap();
        for (file, key) in [
            ("config.js", "config"),
            ("res/data/config_define.js", "define"),
            ("lang/zho-Hans.js", "language"),
        ] {
            let target = root.path().join(file);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, fixture[key].as_str().unwrap()).unwrap();
        }
        root
    })
    .path()
    .to_owned()
}
