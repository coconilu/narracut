import type { StageRun } from "@narracut/contracts";
import type {
  RunArtifactCollection,
  WorkbenchArtifact,
} from "../../../lib/desktop-gateway";
import type { StageView } from "../../../model/workbench";
import {
  StudioEmpty,
  StudioHeading,
  formatBytes,
  runStatusLabels,
  shortHash,
} from "../stage-studio-primitives";

export function OutputView({
  run,
  artifacts,
  loading,
}: {
  readonly run?: StageRun;
  readonly artifacts?: RunArtifactCollection;
  readonly loading: boolean;
}) {
  if (!run) {
    return (
      <StudioEmpty
        title="尚无输出"
        text="选择一个历史运行后查看 Artifact 元数据。"
      />
    );
  }
  return (
    <div className="studio-scroll output-view">
      <StudioHeading
        eyebrow={`${run.runId} · ${runStatusLabels[run.status]}`}
        title="产物清单与来源"
        text="Artifact Store 只返回有界元数据与内容 URI；单次最多读取 24 个产物。"
      />
      {loading ? <div className="studio-inline-empty">正在读取 Artifact 元数据…</div> : null}
      {!loading && artifacts?.items.length ? (
        <div className="artifact-card-grid">
          {artifacts.items.map((artifact) => (
            <ArtifactCard artifact={artifact} key={artifact.artifactId} />
          ))}
        </div>
      ) : null}
      {!loading && !artifacts?.items.length ? (
        <div className="studio-inline-empty">
          该运行没有登记产物，或运行在产物生成前终止。
        </div>
      ) : null}
      {artifacts?.truncated ? (
        <div className="studio-warning">
          共 {artifacts.total} 个产物；当前仅加载前 24 个，避免无界读取。
        </div>
      ) : null}
    </div>
  );
}

function ArtifactCard({ artifact }: { readonly artifact: WorkbenchArtifact }) {
  return (
    <article className={`artifact-card ${artifact.loadError ? "failed" : ""}`}>
      <div className="artifact-card-head">
        <span>{artifact.kind}</span>
        <small>{artifact.mediaType ?? "未知媒体类型"}</small>
      </div>
      <code>{artifact.artifactId}</code>
      {artifact.loadError ? (
        <p>{artifact.loadError}</p>
      ) : (
        <dl>
          <div><dt>来源</dt><dd>{artifact.sourceLabel ?? artifact.sourceOrigin ?? "未知"}</dd></div>
          <div><dt>证据角色</dt><dd>{artifact.evidenceRole ?? "未声明"}</dd></div>
          <div><dt>大小</dt><dd>{formatBytes(artifact.byteLength)}</dd></div>
          <div><dt>内容</dt><dd>{artifact.contentAvailable ? "可用 URI" : "内容缺失"}</dd></div>
        </dl>
      )}
      <div className="trace-chips">
        {artifact.provenance.slice(0, 8).map((item) => (
          <span key={`${item.claimId}:${item.evidenceRef}`}>
            {item.claimId} · {item.evidenceRef}
          </span>
        ))}
        {!artifact.provenance.length ? <span>无 claim/evidence 引用</span> : null}
      </div>
    </article>
  );
}

export function PreviewView({
  run,
  artifacts,
  loading,
  mode,
  stage,
}: {
  readonly run?: StageRun;
  readonly artifacts?: RunArtifactCollection;
  readonly loading: boolean;
  readonly mode: "desktop" | "demo";
  readonly stage: StageView;
}) {
  if (!run) {
    return (
      <StudioEmpty
        title={`${stage.definition.title} 尚无可预览运行`}
        text="第一次运行完成并登记产物后，这里会展示结构化预览入口。"
      />
    );
  }
  const previewArtifact =
    artifacts?.items.find((artifact) => artifact.demoPreview) ?? artifacts?.items[0];
  return (
    <div className="studio-scroll preview-view">
      <div className="preview-meta-row">
        <div>
          <span>{stage.definition.title} · 版本预览</span>
          <strong>{run.runId}</strong>
        </div>
        <span className={`status-pill ${run.status}`}>
          {runStatusLabels[run.status]}
        </span>
      </div>
      {loading ? <div className="studio-inline-empty">正在读取预览元数据…</div> : null}
      {!loading && mode === "demo" && previewArtifact?.demoPreview ? (
        <article className="artifact-preview demo-preview" data-testid="demo-artifact-preview">
          <div className="preview-paper-kicker">演示内容 · {previewArtifact.kind}</div>
          <pre>{previewArtifact.demoPreview}</pre>
          <footer>
            演示内容只用于浏览器模式；桌面模式不会把虚构正文混入真实工程。
          </footer>
        </article>
      ) : null}
      {!loading && mode === "desktop" ? (
        <MetadataPreview artifact={previewArtifact} />
      ) : null}
      {!loading && mode === "demo" && !previewArtifact?.demoPreview ? (
        <StudioEmpty
          title="没有演示预览"
          text="仍可在“输出”页检查真实的产物元数据。"
        />
      ) : null}
    </div>
  );
}

function MetadataPreview({ artifact }: { readonly artifact?: WorkbenchArtifact }) {
  return (
    <article className="artifact-preview metadata-preview">
      <div className="preview-paper-kicker">真实工程 · 元数据预览</div>
      <h2>{artifact?.kind ?? "当前运行没有 Artifact"}</h2>
      <p>
        PR07 的 Artifact Store 命令只返回元数据、contentUri 与 contentAvailable，尚未开放有界字节读取命令。
        因此这里不会伪造正文或直接访问本地文件。
      </p>
      {artifact ? (
        <dl>
          <div><dt>Artifact</dt><dd>{artifact.artifactId}</dd></div>
          <div><dt>Content URI</dt><dd>{artifact.contentUri ?? "未返回"}</dd></div>
          <div><dt>Hash</dt><dd>{shortHash(artifact.contentHash)}</dd></div>
          <div>
            <dt>可用性</dt>
            <dd>{artifact.contentAvailable ? "内容存在，等待安全读取接口" : "内容不可用"}</dd>
          </div>
        </dl>
      ) : null}
    </article>
  );
}
