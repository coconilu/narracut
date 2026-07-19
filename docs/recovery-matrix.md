# Alpha 迁移与恢复矩阵

| 场景 | 检测/动作 | 保留与验收 |
| --- | --- | --- |
| 旧格式/更新格式 | 旧格式显式迁移；新于客户端的格式拒绝打开 | 迁移备份、原项目、迁移报告 |
| 迁移失败 | 临时副本失败即回滚，不替换源目录 | 源 manifest/hash 不变 |
| SQLite 丢失/损坏 | 从项目 JSON、Artifact metadata、Job events 重建 | 数量、owner、hash 与文件真相一致 |
| queued/retrying | 重启扫描后重新调度 | 同 Job/idempotency receipt，不新建历史 |
| running 租约过期/崩溃 | recovery event 转 retrying/failed | 已提交 Artifact journal 可恢复；未完成临时目录不算成功 |
| Render/Export 取消 | 记录 cancel request，整树终止/复制 checkpoint；Export `external_commit` 标记后成功优先 | 标记前回滚 journal/Artifact/`.partial`；标记后拒绝取消并恢复为 succeeded |
| Export rename 后崩溃 | 用稳定 Artifact ID、Manifest 锚与全部文件哈希接管同一 Job | 不遗留不可重试目标；不匹配即冲突 |
| failed/canceled Export journal pending | 启动扫描终态 Export Job，校验无外部提交标记后回滚 | 删除 journal、对应 Artifact 元数据、未引用 CAS、索引与安全 `.partial` |
| succeeded Export journal pending | 启动及项目活动扫描 succeeded ExportResult | Artifact 完整后幂等补记 completed journal |
| 磁盘不足 | 写入前 free-space 和 maxTemporaryBytes 预检 | fail-closed，无最终目录 |
| FFmpeg 缺失/身份漂移 | probe 与冻结 identity 比较 | 原 Render 保留；必须恢复身份或新建运行 |
| 媒体重链接失败 | hash/类型/项目身份不匹配即拒绝 | 原引用不改，不以同名替代 |
| Artifact/输出损坏 | SHA-256、字节数、Manifest verify | 标记 corrupt；历史与诊断保留 |
| 场景局部重生成 | 预览 affected scene IDs，复核未影响输入身份 | 新不可变 Run/Manifest；旧版本可查看/采用 |

自动测试分别位于 `project_service.rs`、`storage_service.rs`、`job_service.rs`、`media_service.rs`、Renderer 测试和 Alpha E2E。发布记录必须列出实际执行命令，不用文档表格替代测试证据。
