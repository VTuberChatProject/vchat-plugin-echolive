//! 真实动态库加载、配置执行、热更新、超时隔离和安全停机验证。
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    time::Duration,
};
use vchat_echolive_config_plugin::{OPERATION, PLUGIN_ID};
#[path = "package_fixture.rs"]
mod fixture;
use vchat_internal_plugin::{InternalPluginManager, PluginError};

/// 由 Cargo 单独生成真实 cdylib，允许 CI 使用预构建动态库。
fn raw_library() -> &'static Path {
    static LIBRARY: OnceLock<PathBuf> = OnceLock::new();
    LIBRARY
        .get_or_init(|| {
            if let Some(path) = std::env::var_os("VCHAT_ECHOLIVE_CONFIG_TEST_LIBRARY") {
                return PathBuf::from(path);
            }
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
                .current_dir(&root)
                .args(["build", "-p", "vchat-echolive-config-plugin", "--lib"])
                .status()
                .expect("构建配置插件动态库");
            assert!(status.success(), "配置插件动态库应成功构建");
            let target = std::env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join("target"));
            target.join("debug").join(format!(
                "{}vchat_echolive_config_plugin{}",
                std::env::consts::DLL_PREFIX,
                std::env::consts::DLL_SUFFIX
            ))
        })
        .as_path()
}

fn library() -> &'static Path {
    static PACKAGE: OnceLock<PathBuf> = OnceLock::new();
    PACKAGE
        .get_or_init(|| fixture::package(raw_library(), None))
        .as_path()
}

fn manager(cache: &Path) -> InternalPluginManager {
    let manager = fixture::manager(cache);
    manager
        .load(fixture::manifest(), library(), json!({}))
        .unwrap();
    manager
}

/// 使用 Node vm 执行实际生成脚本，模拟浏览器 location。
fn execute(script: &str, protocol: &str, host: &str) -> Value {
    execute_location(script, protocol, host, "")
}

fn execute_location(script: &str, protocol: &str, host: &str, search: &str) -> Value {
    let harness = r#"
const vm = require('node:vm');
const fs = require('node:fs');
const input = JSON.parse(fs.readFileSync(0, 'utf8'));
const context = { window: { location: input.location } };
vm.createContext(context);
vm.runInContext(input.script + '\nglobalThis.result = config;', context);
process.stdout.write(JSON.stringify(context.result));
"#;
    let mut child = Command::new("node")
        .args(["-e", harness])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("配置脚本测试需要 Node.js");
    let input = serde_json::to_vec(
        &json!({"script":script,"location":{"protocol":protocol,"host":host,"search":search}}),
    )
    .unwrap();
    child.stdin.take().unwrap().write_all(&input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "JavaScript 执行失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn actual_library_generates_browser_config_and_stops_cleanly() {
    let cache = tempfile::tempdir().unwrap();
    let manager = manager(cache.path());
    let source = "// 保留用户注释\nconst config = {custom: '用户配置', echolive:{broadcast:{}},editor:{websocket:{}}}";
    let response = manager
        .invoke_json(PLUGIN_ID, OPERATION, &json!({"version":1,"source":source}))
        .unwrap();
    assert_eq!(response["version"], 1);
    let script = response["script"].as_str().unwrap();
    assert!(script.starts_with(source), "原始配置完整保留");
    for (protocol, host, expected) in [
        ("http:", "127.0.0.1:38600", "ws://127.0.0.1:38600/ws"),
        ("https:", "localhost", "wss://localhost/ws"),
        ("http:", "localhost", "ws://localhost/ws"),
        ("https:", "[::1]:4430", "wss://[::1]:4430/ws"),
    ] {
        let config = execute(script, protocol, host);
        let broadcast = &config["echolive"]["broadcast"];
        assert_eq!(broadcast["enable"], true);
        assert_eq!(broadcast["websocket_enable"], true);
        assert_eq!(broadcast["websocket_url"], expected);
        assert_eq!(config["editor"]["websocket"]["enable"], true);
        assert_eq!(config["editor"]["websocket"]["url"], expected);
        assert_eq!(config["editor"]["websocket"]["auto_url"], false);
        assert_eq!(
            config["advanced"]["broadcast"]["allow_send_duplicate_message"],
            true
        );
        assert_eq!(config["custom"], "用户配置");
    }
    let preview = execute_location(script, "http:", "localhost", "?other=1&preview=1");
    assert_eq!(
        preview["echolive"]["broadcast"]["websocket_url"],
        "ws://localhost/ws?preview=1"
    );
    assert_eq!(preview["editor"]["websocket"]["url"], "ws://localhost/ws");
    manager
        .reload(PLUGIN_ID, fixture::manifest(), library())
        .unwrap();
    assert!(
        manager
            .invoke_json(PLUGIN_ID, OPERATION, &json!({"version":1,"source":source}))
            .is_ok()
    );
    manager.disable(PLUGIN_ID).unwrap();
    assert!(matches!(
        manager.invoke_json(PLUGIN_ID, OPERATION, &json!({})),
        Err(PluginError::Missing(_))
    ));
}

#[test]
fn actual_library_rejects_protocol_errors() {
    let cache = tempfile::tempdir().unwrap();
    let manager = manager(cache.path());
    for payload in [
        json!({"version":2,"source":"x"}),
        json!({"version":1}),
        json!({"version":1,"source":123}),
    ] {
        assert!(matches!(
            manager.invoke_json(PLUGIN_ID, OPERATION, &payload),
            Err(PluginError::Failed(_))
        ));
    }
    assert!(
        manager
            .invoke_json(PLUGIN_ID, "unknown", &json!({}))
            .is_err()
    );
    manager.disable(PLUGIN_ID).unwrap();
}

#[test]
fn route_registration_conflict_rolls_back_hot_update() {
    let cache = tempfile::tempdir().unwrap();
    let mut manager = fixture::manager(cache.path());
    let reject_routes = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reject = reject_routes.clone();
    manager.set_route_validator(std::sync::Arc::new(move |_, _| {
        !reject.load(std::sync::atomic::Ordering::Acquire)
    }));
    manager
        .load(fixture::manifest(), library(), json!({}))
        .unwrap();
    reject_routes.store(true, std::sync::atomic::Ordering::Release);
    let result = manager.reload(PLUGIN_ID, fixture::manifest(), library());
    assert!(result.is_err(), "候选插件路由冲突时必须拒绝提交");
    let source = "const config = {echolive:{broadcast:{}},editor:{websocket:{}}}";
    let response = manager
        .invoke_json(PLUGIN_ID, OPERATION, &json!({"version":1,"source":source}))
        .unwrap();
    assert!(
        response["script"]
            .as_str()
            .unwrap()
            .contains("InjectConfig")
    );
    manager.disable(PLUGIN_ID).unwrap();
}

#[test]
fn actual_library_timeout_disables_follow_up_calls() {
    let cache = tempfile::tempdir().unwrap();
    let mut manager = manager(cache.path());
    manager.set_timeout(Duration::from_nanos(1));
    let source = "x".repeat(8 * 1024 * 1024);
    assert!(matches!(
        manager.invoke_json(PLUGIN_ID, OPERATION, &json!({"version":1,"source":source})),
        Err(PluginError::Timeout)
    ));
    assert!(!manager.is_active(PLUGIN_ID));
    assert!(
        manager
            .invoke_json(PLUGIN_ID, OPERATION, &json!({"version":1,"source":"x"}))
            .is_err()
    );
    assert_eq!(manager.drain_faults().len(), 1);
}

/// 真实资源定义穿过动态库 ABI，配置保存、拒绝和热更新共享同一有效值。
#[test]
fn native_settings_validate_update_and_survive_hot_reload() {
    let cache = tempfile::tempdir().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Assets/Frontend/Echo-Live");
    let source = std::fs::read_to_string(root.join("config.js")).unwrap();
    let mut config = json!({"echolive_config":{}});
    let manager = fixture::manager(cache.path());
    manager
        .load(fixture::manifest(), library(), config.clone())
        .unwrap();
    let ui = manager.ui_contributions();
    let field = &ui[PLUGIN_ID].fields[0];
    assert_eq!(field.kind, "echolive_config");
    let schema: Value = serde_json::from_str(&field.schema_json).unwrap();
    assert!(schema.get("error").is_none(), "{schema}");
    assert!(schema["fields"].as_array().unwrap().len() > 100);
    assert!(
        schema["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "echolive.broadcast.websocket_url" && f["read_only"] == true)
    );

    config["echolive_config"] = json!({"global.theme":"vanilla","echo.print_speed":47});
    manager.set_config(PLUGIN_ID, &config).unwrap();
    assert_eq!(
        manager
            .invoke_json(PLUGIN_ID, "config.changed", &config)
            .unwrap()["accepted"],
        true
    );
    let script = manager
        .invoke_json(PLUGIN_ID, OPERATION, &json!({"version":1,"source":source}))
        .unwrap();
    let effective = execute(
        script["script"].as_str().unwrap(),
        "http:",
        "localhost:12345",
    );
    assert_eq!(effective["global"]["theme"], "vanilla");
    assert_eq!(effective["echo"]["print_speed"], 47);
    assert_eq!(
        effective["echolive"]["broadcast"]["websocket_url"],
        "ws://localhost:12345/ws"
    );

    let saved_ui = manager.ui_contributions();
    for overrides in [
        json!({"echo.print_speed":-1}),
        json!({"editor.websocket.url":"ws://other"}),
        json!({"unknown.key":true}),
    ] {
        let mut invalid = config.clone();
        invalid["echolive_config"] = overrides;
        assert!(
            manager
                .invoke_json(PLUGIN_ID, "config.changed", &invalid)
                .is_err()
        );
        assert_eq!(manager.ui_contributions()[PLUGIN_ID], saved_ui[PLUGIN_ID]);
    }
    manager
        .reload(PLUGIN_ID, fixture::manifest(), library())
        .unwrap();
    let script = manager
        .invoke_json(PLUGIN_ID, OPERATION, &json!({"version":1,"source":source}))
        .unwrap();
    assert_eq!(
        execute(script["script"].as_str().unwrap(), "https:", "localhost")["echo"]["print_speed"],
        47
    );
    manager.disable(PLUGIN_ID).unwrap();
}

/// 错误配置资源隔离在设置表单，原始脚本生成与连接注入继续可用。
#[test]
fn malformed_resources_disable_form_and_preserve_frontend_generation() {
    let cache = tempfile::tempdir().unwrap();
    let resources = tempfile::tempdir().unwrap();
    std::fs::write(
        resources.path().join("config.js"),
        "const config = broken();",
    )
    .unwrap();
    let manager = fixture::manager(cache.path());
    let archive = fixture::package(raw_library(), Some(resources.path()));
    manager
        .load(fixture::manifest(), &archive, json!({}))
        .unwrap();
    let ui = manager.ui_contributions();
    let schema: Value = serde_json::from_str(&ui[PLUGIN_ID].fields[0].schema_json).unwrap();
    assert!(schema["error"].as_str().is_some());
    let source = "const config = {echolive:{broadcast:{}},editor:{websocket:{}}}";
    let response = manager
        .invoke_json(PLUGIN_ID, OPERATION, &json!({"version":1,"source":source}))
        .unwrap();
    assert_eq!(
        execute(response["script"].as_str().unwrap(), "http:", "localhost")["editor"]["websocket"]
            ["url"],
        "ws://localhost/ws"
    );
    manager.disable(PLUGIN_ID).unwrap();
}
