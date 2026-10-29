# VChat Echo-Live 插件

提供配置管理、网页路由、历史版本适配与播放回执桥接。Echo-Live 运行资源由用户单独获取。

## 构建与导入

安装 Rust 1.99.0；Echo-Live 测试另需 Node.js 22+。

```sh
cargo test --locked
cargo build --locked --release
python Tools/package.py --profile release
```

将 `target/builtin.echolive.vcplug` 在 VChat「设置 → 拓展」中导入并启用。插件通过 C ABI 2 使用 [NativeSDK](https://github.com/VTuberChatProject/VChatPluginNativeSDK)，Cargo 固定 SDK 提交。默认测试仅依赖本仓库和 SDK；真实宿主加载、热更新及播放回归由 [VChat 集成 CI](https://github.com/VTuberChatProject/VChatRev/blob/main/.github/workflows/external-plugins.yml) 按固定版本组合验证。

## Nightly 下载

[Releases](https://github.com/VTuberChatProject/vchat-plugin-echolive/releases) 提供三平台 nightly 插件包。选择与 VChat 运行平台和架构匹配的 `.vcplug`，直接在「设置 → 拓展」中导入。附件格式为 `builtin.echolive-nightly-平台-架构-UTC时间-提交.vcplug`，平台为 `windows`、`macos`、`linux`，架构为对应构建机器的 `x64` 或 `arm64`。

推送到 `main` / `master` 或手动运行 `Plugin` workflow 后，三个平台全部通过格式检查、测试、Clippy、构建和打包，即在本仓库创建 prerelease，成功后清理旧 nightly release 及标签。同一分支运行与发布操作分别串行执行。其他分支 push 和 PR 执行构建检查。

发布使用 GitHub Actions 自带的 `GITHUB_TOKEN`，发布任务声明 `contents: write` 权限。仓库或组织的 Actions 策略需允许该权限。Nightly 用于测试和预览。

## 资源与配置

在插件设置中选择 Echo-Live 文件夹、导入 ZIP 或下载官方资源。配置按资源身份保存，支持官方 data_version 1～17。Live、History、媒体与 WebSocket 路由由启用的插件注册；配置错误通过宿主日志和设置表单报告。单元测试从已跟踪的官方历史夹具生成离线资源。实际 Echo 打印器回归使用 `VCHAT_ECHOLIVE_RESOURCE_ROOT` 显式提供资源，再执行 `cargo test --locked --features external-resource-tests`。

## 消息流连接

Live 与 History 通过 `output_cards_v1` 声明独立端点，按画布连线接收消息。Live 等待打印完成确认；History 由宿主在后台独立归档，页面打开及重连时读取专属记录。页面使用独立 WebSocket 和 BroadcastChannel 通道；预览与清屏携带对应卡片身份。

## 迁移来源

从 `VTuberChatProject/VChatRev` 提交 `985c2a1ae3717f6918cb6030b6dd755d01edff9a` 提取 `Plugins/EchoLiveConfig` 相关历史。路径重映射后的提交保留原作者、日期和说明。许可证见 [LICENSE](LICENSE)。
