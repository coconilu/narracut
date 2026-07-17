import type { StageRun } from "@narracut/contracts";
import type { RunArtifactCollection } from "../../../lib/desktop-gateway";
import {
  Metric,
  StudioEmpty,
  configChangedKeys,
  executorLabel,
  formatUnknown,
  runStatusLabels,
} from "../stage-studio-primitives";

export function CompareView({
  selectedRun,
  compareRun,
  selectedArtifacts,
  compareArtifacts,
}: {
  readonly selectedRun?: StageRun;
  readonly compareRun?: StageRun;
  readonly selectedArtifacts?: RunArtifactCollection;
  readonly compareArtifacts?: RunArtifactCollection;
}) {
  if (!selectedRun || !compareRun) {
    return (
      <StudioEmpty
        title="需要两个历史版本"
        text="当前阶段至少有两个运行后才能比较快照。"
      />
    );
  }
  const changedKeys = configChangedKeys(selectedRun, compareRun);
  return (
    <div className="studio-scroll compare-view">
      <div className="compare-summary">
        <strong>{changedKeys.length} 个配置键变化</strong>
        <span>{selectedRun.artifactIds.length} ↔ {compareRun.artifactIds.length} 个产物</span>
        <span>{selectedRun.inputRefs.length} ↔ {compareRun.inputRefs.length} 个输入</span>
        <span>{selectedRun.logSummary.warnings.length} ↔ {compareRun.logSummary.warnings.length} 个警告</span>
      </div>
      <div className="compare-grid">
        <CompareCard
          label="版本 A"
          run={selectedRun}
          artifacts={selectedArtifacts}
          changedKeys={changedKeys}
        />
        <CompareCard
          label="版本 B"
          run={compareRun}
          artifacts={compareArtifacts}
          changedKeys={changedKeys}
        />
      </div>
    </div>
  );
}

function CompareCard({
  label,
  run,
  artifacts,
  changedKeys,
}: {
  readonly label: string;
  readonly run: StageRun;
  readonly artifacts?: RunArtifactCollection;
  readonly changedKeys: readonly string[];
}) {
  const preview = artifacts?.items.find((artifact) => artifact.demoPreview)?.demoPreview;
  return (
    <article className="compare-card">
      <header>
        <span>{label}</span>
        <strong>{run.runId}</strong>
        <em className={run.status}>{runStatusLabels[run.status]}</em>
      </header>
      <div className="compare-metrics">
        <Metric label="配置" value={`rev ${run.configSnapshot.revision}`} />
        <Metric label="产物" value={`${run.artifactIds.length} 个`} />
        <Metric label="执行器" value={executorLabel(run)} />
      </div>
      <div className="changed-config">
        <strong>变化配置键</strong>
        {changedKeys.length ? (
          changedKeys.map((key) => (
            <div key={key}>
              <code>{key}</code>
              <span>{formatUnknown(run.configSnapshot.values[key])}</span>
            </div>
          ))
        ) : (
          <p>两个版本配置相同。</p>
        )}
      </div>
      <div className="compare-copy">
        {preview ? <pre>{preview}</pre> : <p>{run.logSummary.message}</p>}
      </div>
    </article>
  );
}
