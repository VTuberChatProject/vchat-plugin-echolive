//! EchoLive 的实时和历史输出入口声明。
use vchat_plugin_api::proto::{OutputAction, OutputCard};

/// 页面根路径由宿主按实际监听地址解析，插件仅声明资源归属。
pub(crate) fn cards() -> Vec<OutputCard> {
    [
        (
            "live",
            "Live",
            "实时消息输出",
            "/Echo-Live/live.html",
        ),
        (
            "history",
            "History",
            "历史消息输出",
            "/Echo-Live/history.html",
        ),
    ]
    .into_iter()
    .map(|(id, title, subtitle, target)| OutputCard {
        id: id.into(),
        title: title.into(),
        subtitle: subtitle.into(),
        actions: [
            ("copy", "复制地址", "copy_url"),
            ("open", "浏览器打开", "open_url"),
        ]
        .into_iter()
        .map(|(id, label, kind)| OutputAction {
            id: id.into(),
            label: label.into(),
            kind: kind.into(),
            target: target.into(),
        })
        .collect(),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn live_and_history_have_matching_copy_and_browser_targets() {
        let cards = vchat_plugin_api::output::cards_from_proto(&super::cards()).unwrap();
        assert_eq!(
            cards
                .iter()
                .map(|card| card.title.as_str())
                .collect::<Vec<_>>(),
            ["Live", "History"]
        );
        for (card, path) in cards
            .iter()
            .zip(["/Echo-Live/live.html", "/Echo-Live/history.html"])
        {
            assert_eq!(card.actions.len(), 2);
            assert_eq!(card.actions[0].kind.as_str(), "copy_url");
            assert_eq!(card.actions[1].kind.as_str(), "open_url");
            assert!(card.actions.iter().all(|action| action.target == path));
        }
    }
}
