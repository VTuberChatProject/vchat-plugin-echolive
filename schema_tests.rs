//! 官方字段解析、特殊类型校验和实际浏览器脚本覆盖测试。
use crate::{config::generate_config_with, schema::*, validation::*};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn asset_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Assets/Frontend/Echo-Live")
}

fn field<'a>(schema: &'a Value, path: &str) -> &'a Value {
    schema["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["path"] == path)
        .unwrap()
}

#[test]
fn official_fields_preserve_types_defaults_conditions_and_resource_values() {
    let schema = load_schema(&asset_root()).unwrap();
    let fields = schema["fields"].as_array().unwrap();
    assert!(fields.len() > 100);
    assert!(fields.iter().all(|f| f["kind"] != "object"));
    assert!(
        !fields
            .iter()
            .any(|f| f["path"] == "echolive.print_effect_name"
                || f["path"] == "echolive.next_effect_name")
    );
    let speed = field(&schema, "echo.print_speed");
    assert_eq!(speed["label"], "打印速度");
    assert_eq!(speed["minimum"], 4);
    assert_eq!(speed["default"], 30);
    assert_eq!(field(&schema, "global.theme")["default"], "vanilla");
    let resource = parse_declaration(
        &std::fs::read_to_string(asset_root().join("config.js")).unwrap(),
        "config",
    )
    .unwrap();
    assert_eq!(
        field(&schema, "global.theme")["value"],
        resource["global"]["theme"]
    );
    assert_eq!(
        field(&schema, "editor.form.ontput_before_enable")["kind"],
        "boolean.bit"
    );
    assert_eq!(
        field(&schema, "editor.form.ontput_before_enable")["default"],
        0
    );
    assert_eq!(
        field(&schema, "editor.color_picker.palette")["kind"],
        "special.all_or_array_string"
    );
    assert_eq!(field(&schema, "echolive.print_audio.volume")["maximum"], 1);
    assert_eq!(field(&schema, "echolive.print_audio.volume")["step"], 0.05);
    assert_eq!(
        field(&schema, "editor.form.output_before")["conditions"][0],
        json!({"name":"editor.form.ontput_before_enable","value":1})
    );
    for path in [
        "echolive.broadcast.enable",
        "echolive.broadcast.websocket_enable",
        "echolive.broadcast.websocket_url",
        "editor.websocket.enable",
        "editor.websocket.url",
        "editor.websocket.auto_url",
    ] {
        assert_eq!(field(&schema, path)["read_only"], true);
    }
    assert_eq!(
        field(&schema, "editor.color_picker.contrast_background_color")["input_type"],
        "color"
    );
}

/// 1.9 多轨配置沿用官方类型、标签和依赖，并进入浏览器的最终有效配置。
#[test]
fn multitrack_settings_follow_official_schema_and_browser_overrides() {
    let schema = load_schema(&asset_root()).unwrap();
    for (path, kind, default, label, dependency) in [
        (
            "echolive.print_audio.subtrack_volume",
            "number",
            json!(0.15),
            "子轨道音效音量",
            "echolive.print_audio.enable",
        ),
        (
            "echolive.print_audio.subtrack_rate",
            "number",
            json!(1),
            "子轨道音效播放速度",
            "echolive.print_audio.enable",
        ),
        (
            "echolive.speech_synthesis.speech_subtrack",
            "boolean",
            json!(false),
            "读出子轨道",
            "echolive.speech_synthesis.enable",
        ),
    ] {
        let setting = field(&schema, path);
        assert_eq!(setting["kind"], kind);
        assert_eq!(setting["default"], default);
        assert_eq!(setting["value"], default);
        assert_eq!(setting["label"], label);
        assert_eq!(setting["read_only"], false);
        assert_eq!(
            setting["conditions"],
            json!([{"name": dependency, "value": true}])
        );
    }
    let volume = field(&schema, "echolive.print_audio.subtrack_volume");
    assert_eq!(volume["minimum"], 0);
    assert_eq!(volume["maximum"], 1);
    assert_eq!(volume["step"], 0.05);
    let rate = field(&schema, "echolive.print_audio.subtrack_rate");
    assert_eq!(rate["minimum"], 0);
    assert_eq!(rate["step"], 0.25);
    let overrides = json!({
        "echolive.print_audio.subtrack_volume": 0.35,
        "echolive.print_audio.subtrack_rate": 1.5,
        "echolive.speech_synthesis.speech_subtrack": true,
    });
    validate_overrides(&schema, &overrides).unwrap();
    let source = std::fs::read_to_string(asset_root().join("config.js")).unwrap();
    let result = execute(
        &generate_config_with(&source, Some(&schema), &overrides),
        "http:",
        "localhost",
    );
    assert_eq!(result["config"]["data_version"], 17);
    for (path, expected) in overrides.as_object().unwrap() {
        assert_eq!(path_value(&result["config"], path), Some(expected));
    }
}

#[test]
fn json5_extraction_handles_comments_strings_and_custom_suffix() {
    let source = "// const config = {}\nconst other = 'const config = {}';\nconst /*x*/ config = {\n key: '值 } //', list: ['a',], /* } */ flag: true,\n};\nthrow new Error('资源后缀保持原样');";
    let value = parse_declaration(source, "config").unwrap();
    assert_eq!(value, json!({"key":"值 } //","list":["a"],"flag":true}));
    assert!(
        parse_declaration("const config = fetch('x');", "config")
            .unwrap_err()
            .contains("字面量")
    );
    assert!(
        parse_declaration("const config = {x: 1;};", "config")
            .unwrap_err()
            .contains("config")
    );
    assert!(
        parse_declaration("const config = {x: 1", "config")
            .unwrap_err()
            .contains("未闭合")
    );
    assert!(
        parse_declaration("const config = {x: 'x", "config")
            .unwrap_err()
            .contains("字符串未闭合")
    );
}

#[test]
fn custom_resource_directory_defines_current_fields_and_reports_file_errors() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("res/data")).unwrap();
    std::fs::create_dir_all(root.path().join("lang")).unwrap();
    std::fs::write(root.path().join("res/data/config_define.js"), "const db_config_define = [{name:'global.custom',type:'string',default:'默认',created:1},{name:'global.removed',type:'string',default:'旧',created:1,deleted:2}];").unwrap();
    std::fs::write(
        root.path().join("lang/zho-Hans.js"),
        "const lang_zho_Hans = {config:{global:{custom:{_title:'自定义'}}}};",
    )
    .unwrap();
    std::fs::write(
        root.path().join("config.js"),
        "const config = {data_version:17,global:{custom:'资源'}};\n// 保留自定义代码",
    )
    .unwrap();
    let schema = load_schema(root.path()).unwrap();
    assert_eq!(schema["fields"].as_array().unwrap().len(), 1);
    assert_eq!(field(&schema, "global.custom")["value"], "资源");
    assert_eq!(field(&schema, "global.custom")["label"], "自定义");
    // 资源中的旧类型值仍进入表单，用户可修改草稿或恢复官方默认。
    std::fs::write(
        root.path().join("config.js"),
        "const config = {data_version:17,global:{custom:42}};",
    )
    .unwrap();
    assert_eq!(
        field(&load_schema(root.path()).unwrap(), "global.custom")["value"],
        42
    );
    std::fs::write(root.path().join("config.js"), "const config = { bad: ; }").unwrap();
    let error = load_schema(root.path()).unwrap_err();
    assert!(error.contains("config.js"));
    assert!(error.contains("config"));
}

#[test]
fn validation_enforces_special_types_boundaries_and_host_ownership() {
    let schema = load_schema(&asset_root()).unwrap();
    for valid in [
        json!({}),
        json!({"echo.print_speed":4}),
        json!({"echolive.print_audio.volume":0}),
        json!({"echolive.print_audio.volume":1}),
        json!({"echolive.print_audio.subtrack_volume":0}),
        json!({"echolive.print_audio.subtrack_volume":1}),
        json!({"echolive.print_audio.subtrack_rate":0}),
        json!({"editor.form.ontput_before_enable":1}),
        json!({"editor.color_picker.palette":"all"}),
        json!({"editor.color_picker.palette":["a","b"]}),
        json!({"accessibility.font_size":16}),
    ] {
        validate_overrides(&schema, &valid).unwrap();
    }
    for invalid in [
        json!([]),
        json!({"echo.print_speed":3}),
        json!({"echo.print_speed":"30"}),
        json!({"echolive.print_audio.volume":1.1}),
        json!({"echolive.print_audio.subtrack_volume":-0.1}),
        json!({"echolive.print_audio.subtrack_volume":1.1}),
        json!({"echolive.print_audio.subtrack_rate":-0.25}),
        json!({"echolive.speech_synthesis.speech_subtrack":1}),
        json!({"editor.form.ontput_before_enable":true}),
        json!({"editor.form.ontput_before_enable":2}),
        json!({"editor.color_picker.palette":"other"}),
        json!({"editor.color_picker.palette":[3]}),
        json!({"accessibility.font_size":0}),
        json!({"echolive.broadcast.enable":false}),
        json!({"global.unknown":1}),
        json!({"global.__proto__.polluted":true}),
    ] {
        assert!(
            validate_overrides(&schema, &invalid).is_err(),
            "应拒绝：{invalid}"
        );
    }
}

/// 用 Node VM 运行最终脚本，检查真实 JavaScript 合并行为。
fn execute(script: &str, protocol: &str, host: &str) -> Value {
    let harness = r#"
const vm = require('node:vm');
const fs = require('node:fs');
const input = JSON.parse(fs.readFileSync(0, 'utf8'));
const context = {window:{location:input.location}};
vm.createContext(context);
vm.runInContext(input.script + '\nglobalThis.result = {config, custom: globalThis.custom};', context);
process.stdout.write(JSON.stringify(context.result));
"#;
    let mut child = Command::new("node")
        .args(["-e", harness])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("配置脚本测试需要 Node.js");
    let input =
        serde_json::to_vec(&json!({"script":script,"location":{"protocol":protocol,"host":host}}))
            .unwrap();
    child.stdin.take().unwrap().write_all(&input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "JavaScript 执行失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn generated_script_preserves_source_and_applies_all_four_layers() {
    let schema = load_schema(&asset_root()).unwrap();
    let source = "// 用户资源注释\nconst config = {global:{theme:'资源主题'},echo:{print_speed:45},customKey:'保留'};\nglobalThis.custom = '自定义脚本';";
    let overrides = json!({"echo.print_speed":60,"global.theme":"用户主题"});
    let script = generate_config_with(source, Some(&schema), &overrides);
    assert!(script.starts_with(source));
    for (protocol, host, expected) in [
        ("http:", "localhost:38600", "ws://localhost:38600/ws"),
        ("https:", "[::1]:4430", "wss://[::1]:4430/ws"),
    ] {
        let result = execute(&script, protocol, host);
        let config = &result["config"];
        assert_eq!(config["global"]["theme"], "用户主题");
        assert_eq!(config["global"]["language"], "zho-Hans");
        assert_eq!(config["echo"]["print_speed"], 60);
        assert_eq!(config["customKey"], "保留");
        assert_eq!(result["custom"], "自定义脚本");
        assert_eq!(config["echolive"]["broadcast"]["websocket_url"], expected);
        assert_eq!(config["editor"]["websocket"]["url"], expected);
        assert_eq!(config["editor"]["websocket"]["auto_url"], false);
    }
    let config = execute(
        &generate_config_with(source, Some(&schema), &json!({})),
        "http:",
        "localhost",
    );
    assert_eq!(config["config"]["global"]["theme"], "资源主题");
    assert_eq!(config["config"]["echo"]["print_speed"], 45);
}

#[test]
fn official_reset_overrides_resource_for_every_manageable_field() {
    let schema = load_schema(&asset_root()).unwrap();
    let defaults = schema["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["read_only"] != true)
        .map(|f| (f["path"].as_str().unwrap().to_owned(), f["default"].clone()))
        .collect::<serde_json::Map<_, _>>();
    validate_overrides(&schema, &Value::Object(defaults.clone())).unwrap();
    let source = std::fs::read_to_string(asset_root().join("config.js")).unwrap();
    let result = execute(
        &generate_config_with(&source, Some(&schema), &Value::Object(defaults.clone())),
        "http:",
        "localhost",
    );
    for (path, expected) in defaults {
        assert_eq!(
            path_value(&result["config"], &path),
            Some(&expected),
            "恢复默认：{path}"
        );
    }
}
