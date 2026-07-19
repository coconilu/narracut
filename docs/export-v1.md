# Export、QA 与 Manifest v1

Export v1 是发布边界，不是另一个自由生成阶段。它只采用同一项目里当前、非 stale、完整审核且 SHA-256 复验通过的全片 `rendered_video` 与 `render_log`。

```text
approved Render + ReviewRecord + Renderer identity
  → 11 项 fail-closed QA
  → 冻结 qaHash / Export Job
  → 同卷 .partial 目录
  → Video + WAV + Captions + Timeline + LICENSES + SHA256SUMS
  → manifest.json
  → Artifact journal + 幂等 Artifact import
  → 原子 rename
  → 持久 ExportResult + StageRun
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
- 所有文件先写入受控 `.narracut-<exportId>.partial`，逐文件同步并完成 Artifact journal/import 后才同卷重命名；提交点前失败或取消删除临时目录。
- rename 后若进程崩溃，同一 Job 只能用项目内稳定 `final_video`/`render_manifest` Artifact 哈希锚接管；任何身份或文件不匹配都按目标冲突失败。
- 显式重试只接受 failed/canceled Export Job，并从不可变 receipt 复制业务参数；只替换新 runId 与 idempotencyKey。
- `verify_export` 必须携带 projectId/jobId，并将目录 Manifest 与持久 `ExportResult.manifestHash` 和冻结 Manifest 对比；目录自报哈希不能成为信任根。
- LicenseRecord 指向实际媒体源 Artifact、其 URI/哈希、批准的 Media 文档与真实授权记录 ID；`not_voice_clone` 不是授权记录 ID。
- Manifest 只含项目 URI、相对导出路径、文件名与哈希；不含 Provider Secret、绝对源路径、临时目录或日志全文。

权威契约：`packages/contracts/schema/narracut-export-v1.schema.json`。Tauri 只暴露 `run_export_qa`、`enqueue_export`、`retry_export`、`get_export_result`、`verify_export` 五个有类型命令。
