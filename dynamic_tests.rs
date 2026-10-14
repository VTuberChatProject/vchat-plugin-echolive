//! 真实动态库加载、配置执行、热更新、超时隔离和安全停机验证。
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    time::Duration,
};
use vchat_echolive_config_plugin::{MANIFEST_JSON, OPERATION, PLUGIN_ID};
use vchat_internal_plugin::{InternalPluginManager, PluginError, PluginManifest};

/// 由 Cargo 单独生成真实 cdylib，允许 CI 使用预构建动态库。
fn library() -> &'static Path {
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

fn manager(cache: &Path) -> InternalPluginManager {
    let mut manager = InternalPluginManager::new(cache);
    manager
        .load(
            serde_json::from_str::<PluginManifest>(MANIFEST_JSON).unwrap(),
            library(),
            json!({}),
        )
        .unwrap();
    manager
}

/// 使用 Node vm 执行实际生成脚本，模拟浏览器 location。
fn execute(script: &str, protocol: &str, host: &str) -> Value {
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
        &json!({"script":script,"location":{"protocol":protocol,"host":host}}),
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
    let mut manager = manager(cache.path());
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
        assert_eq!(config["custom"], "用户配置");
    }
    manager
        .reload(
            PLUGIN_ID,
            serde_json::from_str(MANIFEST_JSON).unwrap(),
            library(),
        )
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
    let mut manager = manager(cache.path());
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
fn actual_library_timeout_disables_follow_up_calls() {
    let cache = tempfile::tempdir().unwrap();
    let mut manager = manager(cache.path());
    manager.set_timeout(Duration::from_nanos(1));
    let source = "x".repeat(8 * 1024 * 1024);
    assert!(matches!(
        manager.invoke_json(PLUGIN_ID, OPERATION, &json!({"version":1,"source":source})),
        Err(PluginError::Timeout)
    ));
    assert!(matches!(
        manager.invoke_json(PLUGIN_ID, OPERATION, &json!({"version":1,"source":"x"})),
        Err(PluginError::Timeout)
    ));
}
