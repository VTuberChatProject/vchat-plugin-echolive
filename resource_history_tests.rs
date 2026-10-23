//! 使用固定官方提交的真实页面验证 17 个历史版本的资源校验边界。
use serde_json::Value;
use std::{fs, path::Path};

const CONFIG_FIXTURES: [&str; 17] = [
    include_str!("version_fixture_01.json"),
    include_str!("version_fixture_02.json"),
    include_str!("version_fixture_03.json"),
    include_str!("version_fixture_04.json"),
    include_str!("version_fixture_05.json"),
    include_str!("version_fixture_06.json"),
    include_str!("version_fixture_07.json"),
    include_str!("version_fixture_08.json"),
    include_str!("version_fixture_09.json"),
    include_str!("version_fixture_10.json"),
    include_str!("version_fixture_11.json"),
    include_str!("version_fixture_12.json"),
    include_str!("version_fixture_13.json"),
    include_str!("version_fixture_14.json"),
    include_str!("version_fixture_15.json"),
    include_str!("version_fixture_16.json"),
    include_str!("version_fixture_17.json"),
];

fn write(root: &Path, file: &str, source: &str) {
    let path = root.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, source).unwrap();
}

#[test]
fn accepts_authentic_historical_pages_and_checks_their_version_specific_dependencies() {
    let pages: Value = serde_json::from_str(include_str!("resource_history_pages.json")).unwrap();
    for (index, page) in pages["pages"].as_array().unwrap().iter().enumerate() {
        let fixture: Value = serde_json::from_str(CONFIG_FIXTURES[index]).unwrap();
        assert!(
            page["source"]
                .as_str()
                .unwrap()
                .contains(fixture["commit"].as_str().unwrap())
        );
        let temp = tempfile::tempdir().unwrap();
        // HTML 保持官方原文，依赖文件以非空占位验证文件图；配置定义使用官方原数据。
        for reference in page["dependencies"].as_array().unwrap() {
            write(
                temp.path(),
                reference.as_str().unwrap(),
                "/* 静态资源依赖夹具 */",
            );
        }
        write(temp.path(), "live.html", page["html"].as_str().unwrap());
        write(
            temp.path(),
            "config.js",
            fixture["config"].as_str().unwrap(),
        );
        if let Some(file) = fixture["define_path"].as_str() {
            write(temp.path(), file, fixture["define"].as_str().unwrap());
            write(
                temp.path(),
                "lang/zho-Hans.js",
                fixture["language"].as_str().unwrap(),
            );
        }
        let declared =
            crate::schema::parse_declaration(fixture["config"].as_str().unwrap(), "config")
                .unwrap();
        let descriptor = crate::resources::validate(temp.path())
            .unwrap_or_else(|error| panic!("{}: {error}", fixture["source"]));
        assert_eq!(descriptor["data_version"], declared["data_version"]);
        assert_eq!(descriptor["capabilities"]["live"], true);
        if index == 0 {
            assert!(!temp.path().join("lang").exists());
        }
        let sound = if index < 6 {
            "res/script/sounds.js"
        } else {
            "res/data/sound.js"
        };
        fs::remove_file(temp.path().join(sound)).unwrap();
        assert!(
            crate::resources::validate(temp.path())
                .unwrap_err()
                .contains(sound)
        );
    }
}
