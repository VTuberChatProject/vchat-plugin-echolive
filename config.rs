//! Echo-Live 原始脚本的末尾配置覆盖与同源地址生成。
/// 保留 Echo-Live 原始配置，再在脚本末尾注入同源 WebSocket 覆盖。
pub fn generate_config(source: &str) -> String {
    let mut result = String::with_capacity(source.len() + 1_500);
    result.push_str(source);
    if !source.ends_with('\n') {
        result.push('\n');
    }
    // 显式结束原始脚本最后的语句，兼容官方配置末尾省略分号。
    result.push_str(";\n");
    result.push_str(
        r#"
// VChat 内置配置覆盖：地址跟随当前页面协议、主机和端口。
function InjectConfig() {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsAddress = `${protocol}//${window.location.host}/ws`;
    const preview = new URLSearchParams(window.location.search).get('preview') === '1';
    config.echolive.broadcast.enable = true;
    config.echolive.broadcast.websocket_enable = true;
    config.echolive.broadcast.websocket_url = wsAddress + (preview ? '?preview=1' : '');
    config.echolive.broadcast.channel = preview ? 'vchat:preview' : 'vchat:live';
    config.echolive.broadcast.websocket_reconnect_limit = 1000;
    config.echolive.messages_polling.enable = false;
    config.echolive.sleep.enable = false;
    config.advanced.broadcast.allow_send_duplicate_message = true;
    config.echolive.filter.enable = false;
    config.editor.websocket.enable = true;
    config.editor.websocket.url = wsAddress;
    config.editor.websocket.auto_url = false;
}
InjectConfig();
"#,
    );
    result
}
