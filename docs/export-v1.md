# Export、QA 与 Manifest v1

Export v1 是发布边界，不是另一个自由生成阶段。它只采用同一项目里当前、非 stale、完整审核且 SHA-256 复验通过的全片 `rendered_video` 与 `render_log`。

```text
approved Render + ReviewRecord + Renderer identity
  → 11 项 fail-closed QA
  → 冻结 qaHash / Export Job
  → 同卷 .partial 目录
  → Video + WAV + Captions + Timeline + LICENSES + SHA256SUMS
  → manifest.json
  → 原子 rename
  → final_video / render_manifest Artifact + StageRun
```

## QA Gate

| 类别 | 阻塞规则 |
| --- | --- |
| 画布、时长、音轨 | 与 Timeline 完全一致；时长容差 50 ms；必须可探测音轨 |
| 场景 | ID、顺序、连续覆盖与 RenderResult 一致 |
| 字幕、安全区 | Cue 位于媒体范围；安全区在画布内 |
| 文本布局 | Cue 不重叠、最多 2 行、每行最多 42 字符、无禁止控制字符 |
| 追溯、许可 | claim/evidence 是冻结输入子集；作者、许可、署名、声音授权完整 |
| 哈希、Renderer、Probe | 采用及上游 Artifact 复验；当前 FFmpeg/FFprobe 身份不漂移；MP4 真相完整 |

阻塞项会拒绝入队；警告单独计数。入队后执行前再次计算 QA，`qaHash` 变化即拒绝，避免检查后替换输入。

## 原子性与幂等

- 导出名只能是单个安全目录组件；父目录必须存在且不是符号链接目标。
- 写入前检查目标卷空间与请求临时字节上限；一次只运行一个 Export worker。
- 所有文件先写入受控 `.narracut-<exportId>.partial`，逐文件同步后同卷重命名；失败或取消删除临时目录。
- 已存在目标只在持久化 `ExportResult` 的 `exportId`、Job 与路径完全一致时幂等复用，否则冲突失败。
- Manifest 只含项目 URI、相对导出路径、文件名与哈希；不含 Provider Secret、绝对源路径、临时目录或日志全文。

权威契约：`packages/contracts/schema/narracut-export-v1.schema.json`。Tauri 只暴露 `run_export_qa`、`enqueue_export`、`get_export_result`、`verify_export` 四个有类型命令。
