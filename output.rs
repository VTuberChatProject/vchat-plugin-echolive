//! 在宿主响应中装配剧本输出兼容脚本，保持上游前端资源原样。
pub(crate) const BRIDGE_PATH: &str = "/Echo-Live/vchat-output.js";
pub(crate) const BRIDGE: &str = include_str!("output_bridge.js");

/// 兼容层在 live.js 创建输出实例前按 defer 顺序安装。
pub(crate) fn inject_html(source: &str) -> Option<String> {
    source.find("</head>").map(|index| {
        let mut result = source.to_owned();
        result.insert_str(
            index,
            &format!("<script src=\"{BRIDGE_PATH}\" defer></script>\n"),
        );
        result
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn browser_bridge_handles_preemption_and_stale_callbacks() {
        let output = std::process::Command::new("node")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/output_bridge.test.cjs"
            ))
            .output()
            .expect("浏览器兼容层验证需要 Node.js");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn injects_before_deferred_live_initialization() {
        let source = "<head></head><body><script src=\"res/script/live.js\" defer></script></body>";
        let html = super::inject_html(source).unwrap();
        assert!(html.find(super::BRIDGE_PATH).unwrap() < html.find("res/script/live.js").unwrap());
        assert!(super::inject_html("invalid").is_none());
    }
}
