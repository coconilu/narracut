import type { RendererStageController } from "./use-renderer-stage";

export function RendererStageView({
  controller,
  disabled,
  mode,
}: {
  readonly controller: RendererStageController;
  readonly disabled: boolean;
  readonly mode: "desktop" | "demo";
}) {
  if (mode === "demo") return <RendererDemoFallback />;
  if (!controller.available || !controller.timeline || !controller.config) {
    return (
      <div className="renderer-unavailable" data-testid="renderer-unavailable" role="status">
        <span className="renderer-kicker">RENDERER V1</span>
        <h3>本机渲染尚未就绪</h3>
        <p>{controller.error ?? controller.fallbackReason ?? "需要受支持的 FFmpeg 与当前有效的 approved Timeline。"}</p>
        <button className="button" disabled={disabled} onClick={() => void controller.refresh()} type="button">重新核验</button>
      </div>
    );
  }

  const currentJobId = typeof controller.currentJob?.job.jobId === "string"
    ? controller.currentJob.job.jobId
    : controller.acceptedJob?.jobId;
  const isActive = controller.currentJob
    ? ["queued", "running", "retrying"].includes(controller.currentJob.status)
    : false;
  const canRetry = controller.currentJob
    ? ["failed", "canceled"].includes(controller.currentJob.status)
    : false;

  return (
    <div className="renderer-workbench" data-testid="renderer-workbench">
      <section className="renderer-preview-pane">
        <div className="renderer-section-heading">
          <div><span className="renderer-kicker">ISOLATED SNAPSHOT</span><h3>场景快照</h3></div>
          <select aria-label="选择渲染场景" disabled={disabled || isActive} onChange={(event) => controller.selectScene(event.target.value)} value={controller.selectedSceneId}>
            {controller.timeline.sceneTrack.map((scene, index) => <option key={scene.sceneId} value={scene.sceneId}>#{index + 1} · {scene.sceneId} · {(scene.endMs - scene.startMs) / 1000}s</option>)}
          </select>
        </div>
        <div className="renderer-frame-shell" style={{ aspectRatio: `${controller.config.canvas.width} / ${controller.config.canvas.height}` }}>
          {controller.snapshot ? (
            <iframe
              aria-label={`场景 ${controller.snapshot.sceneId} 的隔离预览`}
              data-testid="renderer-snapshot-frame"
              referrerPolicy="no-referrer"
              sandbox=""
              srcDoc={controller.snapshot.html}
              title="NarraCut deterministic scene snapshot"
            />
          ) : <div className="renderer-frame-loading">正在生成确定性快照…</div>}
        </div>
        {controller.snapshot ? (
          <div className="renderer-snapshot-meta">
            <span>{controller.snapshot.startMs}–{controller.snapshot.endMs} ms</span>
            <code>{controller.snapshot.contentHash}</code>
            <span>{controller.snapshot.claimIds.length} claims · {controller.snapshot.evidenceRefs.length} evidence refs</span>
          </div>
        ) : null}
      </section>

      <aside className="renderer-control-pane">
        <section className="renderer-card">
          <span className="renderer-kicker">FROZEN INPUT</span>
          <h3>已批准 Timeline</h3>
          <dl>
            <div><dt>Run</dt><dd>{controller.timelineInput?.runId}</dd></div>
            <div><dt>Artifact</dt><dd>{controller.timelineInput?.artifactId}</dd></div>
            <div><dt>Review</dt><dd>{controller.timelineInput?.reviewRecordId}</dd></div>
            <div><dt>时长</dt><dd>{(controller.timeline.durationMs / 1000).toFixed(2)} 秒</dd></div>
          </dl>
        </section>

        <section className="renderer-card">
          <span className="renderer-kicker">FIXED CAPABILITY</span>
          <h3>Renderer 身份</h3>
          <dl>
            <div><dt>FFmpeg</dt><dd>{controller.capabilities?.identity.ffmpegVersion}</dd></div>
            <div><dt>Executable</dt><dd>{controller.capabilities?.identity.executableFileName}</dd></div>
            <div><dt>Video / Audio</dt><dd>libx264 / aac</dd></div>
            <div><dt>并发</dt><dd>{controller.capabilities?.limits.maxConcurrentJobs}</dd></div>
          </dl>
          <p className="renderer-security-note">可执行路径、argv、滤镜图和输出路径均不向前端开放。</p>
        </section>

        <section className="renderer-card renderer-config-card">
          <span className="renderer-kicker">ENCODING</span>
          <h3>受限编码配置</h3>
          <label>Preset<select disabled={disabled || isActive} onChange={(event) => controller.setPreset(event.target.value as typeof controller.config.preset)} value={controller.config.preset}>
            {(["veryfast", "faster", "fast", "medium"] as const).map((preset) => <option key={preset} value={preset}>{preset}</option>)}
          </select></label>
          <label>CRF <output>{controller.config.crf}</output><input disabled={disabled || isActive} max="35" min="18" onChange={(event) => controller.setCrf(Number(event.target.value))} type="range" value={controller.config.crf} /></label>
          <div className="renderer-config-summary">{controller.config.canvas.width}×{controller.config.canvas.height} · {controller.config.canvas.frameRateNumerator}/{controller.config.canvas.frameRateDenominator} fps · yuv420p</div>
        </section>

        <section className="renderer-card renderer-actions-card">
          <button className="button" disabled={disabled || isActive} onClick={() => void controller.enqueueScene()} type="button">渲染当前场景</button>
          <button className="button primary" disabled={disabled || isActive} onClick={() => void controller.enqueueTimeline()} type="button">渲染全片</button>
          <button className="button danger" disabled={disabled || !isActive} onClick={() => void controller.cancel()} type="button">取消任务</button>
          <button className="button" disabled={disabled || !canRetry} onClick={() => void controller.retry()} type="button">重试为新运行</button>
        </section>

        {currentJobId ? <section className="renderer-job-card" aria-live="polite">
          <div><strong>{currentJobId}</strong><span>{controller.currentJob?.status ?? "accepted"}</span></div>
          <progress max="1" value={controller.currentJob?.progress ?? 0} />
          <p>{controller.currentJob?.message ?? "等待 Renderer worker…"}</p>
        </section> : null}

        {controller.result ? <section className="renderer-result-card" data-testid="renderer-result">
          <span className="renderer-kicker">IMMUTABLE RESULT</span>
          <h3>{controller.result.target === "timeline" ? "全片" : "场景"}渲染已提交</h3>
          <p>{controller.result.artifacts.length} 个产物 · {controller.result.affectedSceneIds.length} 个受影响场景</p>
          <ul>{controller.result.artifacts.map((artifact) => <li key={artifact.artifactId}><span>{artifact.kind}</span><code>{artifact.contentHash.slice(0, 22)}…</code></li>)}</ul>
        </section> : null}
      </aside>
    </div>
  );
}

function RendererDemoFallback() {
  return (
    <div className="renderer-demo" data-testid="renderer-demo-fallback">
      <div className="renderer-demo-frame" aria-hidden="true">
        <span>NARRACUT / RENDERER V1</span>
        <h3>浏览器演示不会执行本机渲染</h3>
        <div className="renderer-demo-timeline"><i /><i /><i /></div>
      </div>
      <div>
        <span className="renderer-kicker">SAFE FALLBACK</span>
        <h3>请在桌面端连接真实工程</h3>
        <p>此处只展示工作台布局，不会探测 FFmpeg、不会创建 Job，也不会伪造成功产物。桌面端会校验 approved Timeline、内容哈希、ReviewRecord 与 Renderer 身份后才允许渲染。</p>
      </div>
    </div>
  );
}
