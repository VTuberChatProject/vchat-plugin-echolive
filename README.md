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

## 资源与配置

在插件设置中选择 Echo-Live 文件夹、导入 ZIP 或下载官方资源。配置按资源身份保存，支持官方 data_version 1～17。Live、History、媒体与 WebSocket 路由由启用的插件注册；配置错误通过宿主日志和设置表单报告。单元测试从已跟踪的官方历史夹具生成离线资源。

## 迁移来源

从 `VTuberChatProject/VChatRev` 提交 `985c2a1ae3717f6918cb6030b6dd755d01edff9a` 提取 `Plugins/EchoLiveConfig` 相关历史。路径重映射后的提交保留原作者、日期和说明。许可证见 [LICENSE](LICENSE)。
