# v0.1.0 Alpha 已知限制

| 限制 | 影响/规避 |
| --- | --- |
| 仅 Windows x64 验收 | macOS/Linux 未形成安装与 E2E 证据 |
| FFmpeg 不随包提供 | 首次使用前需单独安装受支持 build |
| 未代码签名/自动更新 | SmartScreen 可能提示；仅从可信发布位置取包并核对 SHA-256 |
| 项目代码许可证未确定 | Alpha 只能按明确测试授权分发 |
| 文本布局是确定性规则 | 42 字/行、2 行，不等价于复杂字体的像素级排版引擎 |
| 视觉层为确定性 Scene Snapshot | 没有完整多轨 NLE、Asset Catalog、模板市场或插件 SDK |
| Provider/Renderer 只支持已登记适配器 | 不开放任意 endpoint、shell、FFmpeg 参数或插件安装 |
| 无云协作、账户、遥测后台、SLA | 项目备份、故障收集与分享由用户主动完成 |
| 干净机验收依赖独立 Windows 环境 | 开发机静默安装只算等价流程证据，不能冒充全新 OS 证据 |
