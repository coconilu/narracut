import type { ExportStageController } from "./use-export-stage";

export function ExportStageView({ controller, disabled, mode }: { readonly controller: ExportStageController; readonly disabled: boolean; readonly mode: "desktop" | "demo" }) {
  if (mode === "demo") return <ExportDemoFallback />;
  if (!controller.qaResult || !controller.renderInput) {
    return (
      <div className="export-unavailable" data-testid="export-unavailable" role="status">
        <span className="export-kicker">EXPORT / QA V1</span>
        <h3>尚未形成可导出的批准版本</h3>
        <p>{controller.error ?? controller.fallbackReason ?? "需要非 stale 的 approved Render、闭合哈希与当前 Renderer identity。"}</p>
        <button className="button" disabled={disabled} onClick={() => void controller.refresh()} type="button">重新运行 QA</button>
      </div>
    );
  }

  const qa = controller.qaResult.qa;
  const currentJobId = typeof controller.currentJob?.job.jobId === "string" ? controller.currentJob.job.jobId : controller.acceptedJob?.jobId;
  const isActive = controller.currentJob ? ["queued", "running", "retrying"].includes(controller.currentJob.status) : false;
  const canRetry = controller.currentJob ? ["failed", "canceled"].includes(controller.currentJob.status) : false;

  return (
    <div className="export-workbench" data-testid="export-workbench">
      <section className="export-qa-pane">
        <div className="export-section-heading">
          <div><span className="export-kicker">FAIL-CLOSED QA</span><h3>最终交付检查</h3></div>
          <span className={`export-qa-badge ${qa.status}`}>{qa.passed ? "通过" : `${qa.blockingCount} 项阻塞`}</span>
        </div>
        <div className="export-qa-grid">
          {qa.checks.map((check) => (
            <article className={`export-qa-check ${check.status}`} key={check.checkId}>
              <i aria-hidden="true" /><div><strong>{check.category}</strong><p>{check.message}</p></div>
            </article>
          ))}
        </div>
        <div className="export-hash-line"><span>QA 身份</span><code>{qa.qaHash}</code></div>
      </section>

      <aside className="export-control-pane">
        <section className="export-card">
          <span className="export-kicker">FROZEN INPUT</span><h3>已批准 Render</h3>
          <dl>
            <div><dt>Run</dt><dd>{controller.renderInput.runId}</dd></div>
            <div><dt>Video</dt><dd>{controller.renderInput.artifactId}</dd></div>
            <div><dt>Review</dt><dd>{controller.renderInput.reviewRecordId}</dd></div>
            <div><dt>Claims</dt><dd>{controller.renderInput.claimIds.length} 条</dd></div>
          </dl>
        </section>

        <section className="export-card export-destination-card">
          <span className="export-kicker">ATOMIC DESTINATION</span><h3>导出位置</h3>
          <label><span>目录名</span><input disabled={disabled || isActive} maxLength={64} onChange={(event) => controller.setExportName(event.target.value)} value={controller.exportName} /></label>
          <button className="button" disabled={disabled || isActive} onClick={() => void controller.chooseDestination()} type="button">选择父目录</button>
          <p title={controller.destinationDirectory ?? undefined}>{controller.destinationDirectory ?? "尚未选择；不会写入任何文件。"}</p>
        </section>

        <section className="export-actions-card">
          <button className="button primary" disabled={disabled || !controller.available || !controller.destinationDirectory || isActive} onClick={() => void controller.enqueue()} type="button">导出交付包</button>
          <button className="button danger" disabled={disabled || !isActive} onClick={() => void controller.cancel()} type="button">取消</button>
          <button className="button" disabled={disabled || !canRetry} onClick={() => void controller.retry()} type="button">新运行重试</button>
          <button className="button" disabled={disabled || !controller.result} onClick={() => void controller.verify()} type="button">复验哈希</button>
        </section>

        {currentJobId ? (
          <section className="export-job-card">
            <div><strong>{currentJobId}</strong><span>{controller.currentJob?.status ?? controller.acceptedJob?.status}</span></div>
            <progress max={1} value={controller.currentJob?.progress ?? 0} />
            <p>{controller.currentJob?.message ?? "等待 worker 接管…"}</p>
          </section>
        ) : null}

        {controller.result ? (
          <section className="export-result-card">
            <span className="export-kicker">PORTABLE RESULT</span><h3>交付包已生成</h3>
            <p title={controller.result.exportPath}>{controller.result.exportPath}</p>
            <dl>
              <div><dt>Manifest</dt><dd>{controller.result.manifestHash}</dd></div>
              <div><dt>文件</dt><dd>{controller.result.manifest.files.length} 项</dd></div>
              <div><dt>完整性</dt><dd>{controller.verification?.status ?? "待复验"}</dd></div>
            </dl>
          </section>
        ) : null}
      </aside>
    </div>
  );
}

function ExportDemoFallback() {
  return (
    <div className="export-demo" data-testid="export-demo">
      <div><span className="export-kicker">NARRACUT / EXPORT V1</span><h3>浏览器演示不会伪造导出成功</h3><div className="export-demo-files"><i /><i /><i /><i /></div></div>
      <div><span className="export-kicker">SAFE FALLBACK</span><h3>请在桌面端连接真实工程</h3><p>桌面端会重新探测 FFmpeg、核验 approved Render 与 SHA-256，再通过异步 Job 原子生成视频、音频、字幕、时间轴、许可、校验和与 Manifest。</p></div>
    </div>
  );
}
