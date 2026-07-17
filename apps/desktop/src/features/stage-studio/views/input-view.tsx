import type { InputReference, StageRun } from "@narracut/contracts";
import {
  Metric,
  StudioEmpty,
  StudioHeading,
  executorLabel,
  shortHash,
} from "../stage-studio-primitives";

export function InputView({ run }: { readonly run?: StageRun }) {
  if (!run) {
    return (
      <StudioEmpty
        title="尚无运行输入"
        text="阶段第一次运行后，这里会展示冻结的输入引用与审核链。"
      />
    );
  }
  return (
    <div className="studio-scroll studio-input-view">
      <StudioHeading
        eyebrow={`${run.runId} · 不可变执行快照`}
        title="输入引用与审核链"
        text="这里展示运行当时冻结的输入，不会用当前项目资料反向改写历史。"
      />
      <div className="snapshot-strip">
        <Metric label="输入哈希" value={shortHash(run.inputHash)} mono />
        <Metric label="配置修订" value={`rev ${run.configSnapshot.revision}`} />
        <Metric label="执行器" value={executorLabel(run)} />
        <Metric label="输入数量" value={`${run.inputRefs.length} 个`} />
      </div>
      <div className="input-reference-list">
        {run.inputRefs.length > 0 ? (
          run.inputRefs.map((reference) => (
            <InputReferenceCard key={reference.refId} reference={reference} />
          ))
        ) : (
          <div className="studio-inline-empty">
            该运行没有上游 Artifact 引用，直接消费项目资料或空输入。
          </div>
        )}
      </div>
    </div>
  );
}

function InputReferenceCard({
  reference,
}: {
  readonly reference: InputReference;
}) {
  const artifact = reference.referenceType === "artifact";
  return (
    <article className="reference-card">
      <div className="reference-card-head">
        <span>{reference.kind}</span>
        <code>{reference.refId}</code>
      </div>
      <dl>
        <div><dt>引用类型</dt><dd>{artifact ? "Artifact" : "项目文档"}</dd></div>
        <div><dt>来源</dt><dd>{artifact ? reference.artifactId : reference.uri}</dd></div>
        <div>
          <dt>审核链</dt>
          <dd>
            {artifact
              ? `${reference.sourceRunId} · ${reference.reviewRecordId}`
              : "直接项目输入"}
          </dd>
        </div>
        <div><dt>内容哈希</dt><dd><code>{shortHash(reference.contentHash)}</code></dd></div>
      </dl>
      <div className="trace-chips">
        {reference.claimIds.map((claimId) => <span key={claimId}>{claimId}</span>)}
        {reference.evidenceRefs.map((evidenceRef) => <span key={evidenceRef}>{evidenceRef}</span>)}
        {reference.claimIds.length + reference.evidenceRefs.length === 0 ? (
          <span>无事实引用</span>
        ) : null}
      </div>
    </article>
  );
}
