//! 官方 data_version 1～17 离线夹具与 Node VM 配置覆盖回归。
use crate::{config::generate_config_with, schema::*, validation::*};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

const FIXTURES: [&str; 17] = [
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

fn materialize(root: &Path, fixture: &Value) {
    std::fs::write(root.join("config.js"), fixture["config"].as_str().unwrap()).unwrap();
    if let Some(file) = fixture["define_path"].as_str() {
        std::fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
        std::fs::write(root.join(file), fixture["define"].as_str().unwrap()).unwrap();
        std::fs::create_dir_all(root.join("lang")).unwrap();
        std::fs::write(
            root.join("lang/zho-Hans.js"),
            fixture["language"].as_str().unwrap(),
        )
        .unwrap();
    }
}

fn has(schema: &Value, path: &str) -> bool {
    schema["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field["path"] == path)
}

/// 以真实 JavaScript 执行结果检查旧路径、原始声明、自定义代码及受控连接。
fn execute(script: &str) -> Value {
    let harness = r#"
const vm = require('node:vm');
const fs = require('node:fs');
const context = {window:{location:{protocol:'https:',host:'localhost:38600',search:'?preview=1'}}};
vm.createContext(context);
vm.runInContext(fs.readFileSync(0,'utf8') + '\nglobalThis.result = {config, marker:globalThis.marker}', context);
process.stdout.write(JSON.stringify(context.result));
"#;
    let mut child = Command::new("node")
        .args(["-e", harness])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn all_official_versions_keep_version_defaults_paths_and_custom_javascript() {
    for (index, fixture) in FIXTURES.iter().enumerate() {
        let fixture: Value = serde_json::from_str(fixture).unwrap();
        let root = tempfile::tempdir().unwrap();
        materialize(root.path(), &fixture);
        let schema =
            load_schema(root.path()).unwrap_or_else(|e| panic!("fixture {}: {e}", index + 1));
        let expected = if index == 11 { 11 } else { index + 1 };
        assert_eq!(schema["data_version"], expected);
        assert_eq!(schema["effective_version"], index + 1);
        assert_eq!(schema["read_only"], false);
        assert!(schema["fields"].as_array().unwrap().len() > 30);
        let source = format!(
            "{}\nglobalThis.marker = '保留自定义';",
            fixture["config"].as_str().unwrap()
        );
        let overrides = json!({"echo.print_speed": 42});
        validate_overrides(&schema, &overrides).unwrap();
        let script = generate_config_with(&source, Some(&schema), &overrides);
        assert!(script.starts_with(&source));
        let result = execute(&script);
        assert_eq!(result["config"]["data_version"], expected);
        assert_eq!(result["config"]["echo"]["print_speed"], 42);
        assert_eq!(result["marker"], "保留自定义");
        let config = &result["config"];
        if index < 3 {
            assert_eq!(
                config["echolive"]["websocket_url"],
                "wss://localhost:38600/ws?preview=1"
            );
            assert_eq!(config["echolive"]["websocket_enable"], true);
            assert!(config["echolive"].get("broadcast").is_none());
        } else {
            assert_eq!(
                config["echolive"]["broadcast"]["websocket_url"],
                "wss://localhost:38600/ws?preview=1"
            );
            assert_eq!(config["echolive"]["broadcast"]["websocket_enable"], true);
        }
        if index < 9 {
            assert!(config["editor"].get("websocket").is_none());
        } else {
            assert_eq!(
                config["editor"]["websocket"]["url"],
                "wss://localhost:38600/ws"
            );
            assert_eq!(config["editor"]["websocket"]["auto_url"], false);
        }
        let defaults = schema["fields"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|field| field["read_only"] != true)
            .map(|field| {
                (
                    field["path"].as_str().unwrap().to_owned(),
                    field["default"].clone(),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        validate_overrides(&schema, &Value::Object(defaults)).unwrap();
        assert!(!has(&schema, "echolive.print_effect_name"));
        assert_eq!(has(&schema, "accessible.high_contrast"), index < 9);
        assert_eq!(has(&schema, "accessibility.high_contrast"), index >= 9);
    }
}

#[test]
fn unknown_or_missing_versions_stay_read_only_and_keep_source_exactly() {
    for literal in [
        "{}",
        "{data_version:18}",
        "{data_version:0}",
        "{data_version:'17'}",
    ] {
        let source = format!("const config = {literal}; // 保留末行");
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("config.js"), &source).unwrap();
        let schema = load_schema(root.path()).unwrap();
        assert_eq!(schema["read_only"], true);
        assert!(schema["fields"].as_array().unwrap().is_empty());
        assert!(!schema["compatibility"].as_str().unwrap().is_empty());
        assert_eq!(
            generate_config_with(&source, Some(&schema), &json!({})),
            source
        );
        assert_eq!(crate::generate_config(&source), source);
        validate_overrides(&schema, &json!({})).unwrap();
        assert!(validate_overrides(&schema, &json!({"echo.print_speed":40})).is_err());
    }
}

#[test]
fn field_lifetimes_and_from_respect_resource_version() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("res/data")).unwrap();
    std::fs::create_dir_all(root.path().join("lang")).unwrap();
    std::fs::write(
        root.path().join("config.js"),
        "const config={data_version:3,echolive:{broadcast_channel:'旧频道'}};",
    )
    .unwrap();
    std::fs::write(
        root.path().join("lang/zho-Hans.js"),
        "const lang_zho_Hans={};",
    )
    .unwrap();
    std::fs::write(root.path().join("res/data/config_define.js"), r#"const db_config_version=17;
const db_config_define=[
{name:'echolive.broadcast.channel',from:'echolive.broadcast_channel',created:1,type:'string',default:'默认'},
{name:'echo.deleted_later',created:1,deleted:4,type:'boolean',default:true},
{name:'echo.deleted_now',created:1,deleted:3,type:'boolean',default:true},
{name:'echo.future',created:4,type:'boolean',default:true},
{name:'echo.reserved',created:-1,type:'boolean',default:true}];"#).unwrap();
    let schema = load_schema(root.path()).unwrap();
    assert!(has(&schema, "echolive.broadcast_channel"));
    assert!(has(&schema, "echo.deleted_later"));
    assert!(!has(&schema, "echo.deleted_now"));
    assert!(!has(&schema, "echo.future"));
    assert!(!has(&schema, "echo.reserved"));
}

#[test]
fn saved_inactive_values_can_survive_without_accepting_new_unknown_fields() {
    let root = tempfile::tempdir().unwrap();
    materialize(
        root.path(),
        &serde_json::from_str::<Value>(FIXTURES[16]).unwrap(),
    );
    let schema = load_schema(root.path()).unwrap();
    let previous = json!({"accessible.high_contrast":true,"echo.print_speed":40});
    validate_overrides_preserving(&schema, &previous, &previous).unwrap();
    assert!(
        validate_overrides_preserving(
            &schema,
            &json!({"accessible.high_contrast":false}),
            &previous
        )
        .is_err()
    );
    assert!(
        validate_overrides_preserving(&schema, &json!({"arbitrary.new":42}), &previous).is_err()
    );
    assert_eq!(
        compatible_overrides(&schema, &previous),
        json!({"accessibility.high_contrast":true,"echo.print_speed":40})
    );
    assert_eq!(
        compatible_overrides(
            &schema,
            &json!({"echo.print_speed":"坏类型","echolive.broadcast.enable":false,"arbitrary.new":1})
        ),
        json!({})
    );
    let obsolete = json!({"echo.print_speed":"历史旧类型","echolive.broadcast.enable":false});
    validate_overrides_preserving(&schema, &obsolete, &obsolete).unwrap();
    let result = execute(&generate_config_with(
        FIXTURES[16].parse::<Value>().unwrap()["config"]
            .as_str()
            .unwrap(),
        Some(&schema),
        &obsolete,
    ));
    assert_eq!(result["config"]["echo"]["print_speed"], 30);
    assert_eq!(result["config"]["echolive"]["broadcast"]["enable"], true);
    let result = execute(&generate_config_with(
        FIXTURES[16].parse::<Value>().unwrap()["config"]
            .as_str()
            .unwrap(),
        Some(&schema),
        &previous,
    ));
    assert!(result["config"].get("accessible").is_none());
    assert_eq!(result["config"]["echo"]["print_speed"], 40);
}

#[test]
fn version_12_also_accepts_explicit_declared_version_and_preserves_it() {
    let mut fixture: Value = serde_json::from_str(FIXTURES[11]).unwrap();
    let config = parse_declaration(fixture["config"].as_str().unwrap(), "config").unwrap();
    let mut config = config;
    config["data_version"] = json!(12);
    fixture["config"] = json!(format!("const config={config};"));
    let root = tempfile::tempdir().unwrap();
    materialize(root.path(), &fixture);
    let schema = load_schema(root.path()).unwrap();
    assert_eq!(schema["data_version"], 12);
    assert_eq!(schema["effective_version"], 12);
    assert!(has(&schema, "global.theme_variant"));
    assert_eq!(
        execute(&generate_config_with(
            fixture["config"].as_str().unwrap(),
            Some(&schema),
            &json!({})
        ))["config"]["data_version"],
        12
    );
}

#[test]
fn externally_replaced_config_waits_for_resource_version_revalidation() {
    let root = tempfile::tempdir().unwrap();
    materialize(
        root.path(),
        &serde_json::from_str::<Value>(FIXTURES[16]).unwrap(),
    );
    let schema = load_schema(root.path()).unwrap();
    for config in [
        FIXTURES[0].parse::<Value>().unwrap()["config"]
            .as_str()
            .unwrap(),
        "const config={data_version:18};",
    ] {
        assert_eq!(
            generate_config_with(config, Some(&schema), &json!({"echo.print_speed":40})),
            config
        );
    }
}
