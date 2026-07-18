# NarraCut 媒体服务 v1

## 1. 能力边界

PR10 建立第一条真实媒体链，但不把 NarraCut 变成黑盒成片生成器：

```text
已审核 Script
  -> PCM WAV 导入 -> Audio 候选运行
  -> UTF-8 SRT 导入 -> Captions 候选运行
已审核 Research + Script + Captions
  -> Scene Plan 生成/编辑
已审核 Audio + Captions + Scene Plan
  -> Timeline 生成/编辑 -> PR11 Renderer 唯一输入
```

每次导入、生成或编辑都绑定新的 `runId`。历史 `StageRun`、Artifact、审核记录和
上层请求 receipt 不会被覆盖；只有明确审核通过的运行才进入下游输入。

本版本不提供 MP3/AAC、自动转写、强制对齐、TTS、声音克隆、波形编辑、混音、视频预览
或渲染。词级时间是由 SRT cue 内插得到的估算值，不能表述为转写或精确对齐结果。

## 2. 契约与命令

| 边界 | 权威 Schema | 主要内容 |
| --- | --- | --- |
| 媒体文档 | `narracut-media-v1.schema.json` | `AudioMediaDocument`、`CaptionsMediaDocument`、`ScenePlanDocument`、`TimelineDocument` |
| 媒体命令 | `narracut-media-commands-v1.schema.json` | 导入、查询、生成、编辑保存及结构化错误 |
| 任务生命周期 | `narracut-job-commands-v1.schema.json` | 查询、进度、取消、人工新运行重试与恢复 |

React 只调用有类型的高层 Tauri command。外部路径只存在于一次导入请求的内存边界，
不会写入项目、SQLite、Job、StageRun、Artifact 元数据或用户可见错误。前端不能传递任意
shell、FFmpeg 参数或插件操作。

## 3. 导入与项目内身份

导入命令先在 Rust 边界检查项目身份、来源类型和资源上限，再把外部文件流式复制到：

```text
requests/media-sources/sha256/<hash-prefix>/<sha256>/<safe-file-name>
```

该 URI、SHA-256、字节数和安全文件名组成冻结来源身份。执行时 worker 重新校验内容哈希
和长度；外部文件随后变化不会改变已入队任务。相同内容可复用项目内 staged source，
不同内容不能用同一个 receipt 或幂等键静默替换。

来源必须是普通文件。目录、设备、symlink/reparse 逃逸、项目外写入、遍历片段和非法
项目内 URI 都会 fail-closed。临时内容使用同项目文件系统的无覆盖原子提交；失败或取消
不会留下可被误认为完成 Artifact 的半成品。

## 4. 格式与资源上限

| 输入 | Alpha 支持 | 默认/硬上限 |
| --- | --- | ---: |
| 音频 | RIFF/WAVE，格式码 1 的未压缩 PCM；1–8 声道；8/16/24/32 bit；8,000–384,000 Hz | 64 MiB；最长 24 小时 |
| 字幕 | UTF-8，可选 BOM；LF 或 CRLF；严格顺序的 SRT cue | 默认 4 MiB；最多 10,000 cue；单 cue 最多 2,000 字符/8,000 UTF-8 字节 |
| Scene Plan | 稳定 `sceneId`，有序且覆盖合法时间范围 | 最多 10,000 场景；单次最多 1,000 个编辑 |
| Timeline | 1 条音频引用轨、1 条场景轨、1 条字幕引用轨 | 最多 10,000 场景/cue；画布边长 16–16,384；帧率不超过 240 fps |

WAV 解析不信任扩展名或 MIME；它校验 RIFF/WAVE、chunk 长度、PCM 格式、`blockAlign`、
`byteRate`、data 帧对齐和整数溢出。SRT 校验索引、时间格式、`start < end`、排序、重叠、
空文本及音频时长范围；不会静默裁剪、重排或改写错误 cue。

## 5. 审核、追溯与权利

`MediaReviewedInputReference` 必须同时绑定：

| 字段 | 校验 |
| --- | --- |
| `stageId + runId + artifactId` | 当前项目、正确阶段、真实运行和产物 |
| `contentHash` | 与 Artifact Store 中的内容身份一致 |
| `reviewRecordId` | 对该运行和产物的 `approved` 审核记录 |
| `claimIds + evidenceRefs` | 是已审核 Artifact 追溯集合的合法子集 |

缺少审核、输入 stale、哈希不一致、跨项目引用或追溯不完整都拒绝执行。Captions 的 cue、
Scene Plan 的场景和 Timeline 的字幕引用继续保留 `claimId` / `evidenceRef`；阻塞诊断未解决
时不得审核通过。

音频和字幕导入还必须保存作者、权利声明、许可证、署名文字和内容哈希。音频权利只能
声明为本人录制或已获许可，`voiceAuthorization` 固定为 `not_voice_clone`，不能借此冒充
声音克隆授权。生成式图像或视频只能是表达素材，不能成为事实证据。

## 6. 时间精度与编辑

| 映射层级 | `timingPrecision` | `timingBasis` |
| --- | --- | --- |
| cue | `cue_exact` | `srt_cue` |
| sentence | `estimated` | `sentence_interpolation` |
| word | `estimated` | `word_interpolation` |

Scene Plan 支持拆分、合并相邻场景、修改标题/叙事职责和移动相邻边界。Timeline 支持移动
相邻场景边界、设置画布内安全区和字幕引用轨可见性。保存前会一次性校验编辑集合；成功后
写入新的 Artifact/StageRun，并记录 `supersedesArtifactId`、变更摘要和
`changedSceneIds`。旧版本仍可查看、比较、审核和重新采用。

Timeline 必须完整覆盖音频时长，场景有序、非负、`start < end`，默认无空洞或重叠；
安全区必须完全位于画布内。PR11 Renderer 只消费已经审核通过的 `TimelineDocument`，不得
绕过它从原始音频、字幕或资料重新总结。

## 7. Job、取消、重试与恢复

音频/SRT 导入以及 Scene Plan/Timeline 生成属于持久化媒体 Job：

| 行为 | 语义 |
| --- | --- |
| 入队 | 立即返回 `jobId`，冻结 request receipt、输入、配置、执行器和重试策略 |
| 进度 | worker 报告单调进度；当前实现的主要安全边界为 10%、35%、85% 和终态 |
| 自动重试 | 同一未终结运行最多 3 次，1 秒起始、2 倍退避、最多 15 秒 |
| 人工重试 | 仅允许 failed/canceled 媒体任务；复制原 receipt 的业务字段，只替换新 `runId` 与幂等键 |
| 取消 | queued 立即取消；running 在当前有界副作用结束后记录已产生 Artifact，再确认 canceled |
| 恢复 | 启动、打开项目或显式恢复时，重放事件、处理过期租约，并按冻结 receipt 重新调度 |

媒体执行器身份固定为本地 `narracut_media_runtime`。它不会领取 Provider、通用或其他
执行器的任务；Provider worker 同样不会领取媒体任务。运行中的取消不会让后台线程继续
静默写入；worker 只有确认安全边界后才提交终态。

## 8. 故障排查

| 现象 | 先检查 | 正确处理 |
| --- | --- | --- |
| WAV 被拒绝 | 是否真的是未压缩 PCM；声道、采样率、位深和 data chunk 是否合法 | 转换为受支持 PCM WAV 后创建新导入，不要只改扩展名 |
| SRT 被拒绝 | UTF-8、连续索引、时间格式、重叠和音频范围 | 修复源文件并创建新导入；不要依赖静默裁剪 |
| “需要审核” | 上游阶段是否有 approved run，审核是否包含所选 Artifact | 在 Stage Studio 审核明确产物后重试 |
| “输入已过期/哈希不一致” | 上游是否重导入、改配置或重新采用 | 查看 stale 原因，选择最新已审核输入并创建新运行 |
| 任务 canceled/failed | Job 事件、可重试标记和来源 staged 内容是否仍完整 | 使用“重试”创建新 run；不要修改旧 Job/StageRun |
| 应用重启后任务未继续 | 项目是否仍在最近项目索引、任务 receipt 是否完整 | 打开项目或执行恢复扫描；receipt 损坏时保留失败证据并重新导入 |
| 浏览器演示模式无法选文件 | 浏览器 fallback 没有本机文件权限 | 使用 Tauri 桌面端；演示模式不会伪造导入成功 |

验证媒体核心与桌面边界：

```powershell
pnpm --filter @narracut/contracts test
pnpm --filter @narracut/desktop test
cargo test -p narracut-core --test media_service
cargo test -p narracut media_commands::tests
```
