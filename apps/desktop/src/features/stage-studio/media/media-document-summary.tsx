import type {
  AudioMediaDocument,
  CaptionCue,
  CaptionsMediaDocument,
  MediaDiagnostic,
} from "@narracut/contracts";
import { formatDuration } from "./media-stage-model.js";

export interface MediaDocumentSummaryProps {
  readonly document: AudioMediaDocument | CaptionsMediaDocument | null;
  readonly stageId: "audio" | "captions";
}

interface MappingSummary {
  readonly cueExact: number;
  readonly estimated: number;
  readonly srtCue: number;
  readonly sentenceInterpolation: number;
  readonly wordInterpolation: number;
}

interface DiagnosticSummary {
  readonly info: number;
  readonly warning: number;
  readonly error: number;
  readonly blocking: readonly MediaDiagnostic[];
}

function sourceFileName(value: string): string {
  const segments = value.split(/[\\/]/).filter(Boolean);
  return segments[segments.length - 1]?.trim() || "未命名来源文件";
}

function redactLocalPaths(value: string): string {
  return value
    .replace(/[a-z]:[\\/][^\s，。；；]+/gi, "[本地路径已隐藏]")
    .replace(/\\\\[^\s，。；；]+/g, "[本地路径已隐藏]")
    .replace(/(^|[\s(])\/(?!\/)[^\s，。；；)]+/g, "$1[本地路径已隐藏]");
}

function formatBytes(value: number): string {
  return `${new Intl.NumberFormat("zh-CN").format(value)} bytes`;
}

function summarizeMappings(
  mappings: CaptionsMediaDocument["mappings"],
): MappingSummary {
  let cueExact = 0;
  let estimated = 0;
  let srtCue = 0;
  let sentenceInterpolation = 0;
  let wordInterpolation = 0;
  for (const mapping of mappings) {
    if (mapping.timingPrecision === "cue_exact") cueExact += 1;
    else estimated += 1;
    if (mapping.timingBasis === "srt_cue") srtCue += 1;
    else if (mapping.timingBasis === "sentence_interpolation") {
      sentenceInterpolation += 1;
    } else {
      wordInterpolation += 1;
    }
  }
  return {
    cueExact,
    estimated,
    srtCue,
    sentenceInterpolation,
    wordInterpolation,
  };
}

function summarizeDiagnostics(
  diagnostics: readonly MediaDiagnostic[],
): DiagnosticSummary {
  let info = 0;
  let warning = 0;
  let error = 0;
  const blocking: MediaDiagnostic[] = [];
  for (const diagnostic of diagnostics) {
    if (diagnostic.severity === "info") info += 1;
    else if (diagnostic.severity === "warning") warning += 1;
    else error += 1;
    if (diagnostic.blocking) blocking.push(diagnostic);
  }
  return { info, warning, error, blocking };
}

function TraceValues({
  emptyLabel,
  values,
}: {
  readonly emptyLabel: string;
  readonly values: readonly string[];
}) {
  if (values.length === 0) return <span>{emptyLabel}</span>;
  return (
    <span className="media-trace-values">
      {values.map((value) => (
        <code key={value}>{redactLocalPaths(value)}</code>
      ))}
    </span>
  );
}

function AudioDocumentSummary({
  document,
}: {
  readonly document: AudioMediaDocument;
}) {
  const rights = document.rights;
  return (
    <section
      aria-labelledby="media-audio-summary-title"
      className="media-document-card media-audio-summary"
      data-testid="media-audio-summary"
    >
      <header className="media-document-heading">
        <div>
          <h2 id="media-audio-summary-title">正式音频文档</h2>
          <p>展示可追踪媒体属性与授权记录，不暴露本机绝对路径。</p>
        </div>
      </header>
      <dl className="media-document-metadata">
        <dt>时长</dt>
        <dd>
          {formatDuration(document.durationMs)}（{document.durationMs} ms）
        </dd>
        <dt>采样率</dt>
        <dd>{document.sampleRateHz} Hz</dd>
        <dt>声道</dt>
        <dd>{document.channels}</dd>
        <dt>位深</dt>
        <dd>{document.bitsPerSample} bit</dd>
        <dt>来源文件</dt>
        <dd>{sourceFileName(document.source.sourceFileName)}</dd>
        <dt>来源哈希</dt>
        <dd><code>{document.source.sourceContentHash}</code></dd>
        <dt>来源大小</dt>
        <dd>{formatBytes(document.source.byteLength)}</dd>
        <dt>权利类型</dt>
        <dd>{rights.ownership === "self_recorded" ? "自行录制或制作" : "已取得许可"}</dd>
        <dt>作者或录制者</dt>
        <dd>{redactLocalPaths(rights.author)}</dd>
        <dt>权利声明</dt>
        <dd>{redactLocalPaths(rights.rightsStatement)}</dd>
        <dt>许可编号</dt>
        <dd>{redactLocalPaths(rights.licenseId) || "无"}</dd>
        <dt>署名文本</dt>
        <dd>{redactLocalPaths(rights.attributionText) || "无需署名"}</dd>
        <dt>声音授权</dt>
        <dd>非声音克隆</dd>
      </dl>
    </section>
  );
}

function CueTable({ cues }: { readonly cues: readonly CaptionCue[] }) {
  return (
    <div
      aria-label="字幕 cue 明细，可横向和纵向滚动"
      className="media-table-scroll media-cue-table-scroll"
      role="region"
      tabIndex={0}
    >
      <table className="media-cue-table" data-testid="media-cue-table">
        <caption>字幕 cue 与事实追溯</caption>
        <thead>
          <tr>
            <th scope="col">序号</th>
            <th scope="col">cue_id</th>
            <th scope="col">起止时间</th>
            <th scope="col">字幕文本</th>
            <th scope="col">claim_id</th>
            <th scope="col">evidence_ref</th>
          </tr>
        </thead>
        <tbody>
          {cues.map((cue) => (
            <tr key={cue.cueId}>
              <td>{cue.sourceIndex}</td>
              <td><code>{cue.cueId}</code></td>
              <td>
                {formatDuration(cue.startMs)} – {formatDuration(cue.endMs)}
              </td>
              <td>{redactLocalPaths(cue.text)}</td>
              <td>
                <TraceValues emptyLabel="无" values={cue.claimIds} />
              </td>
              <td>
                <TraceValues emptyLabel="无" values={cue.evidenceRefs} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function BlockingDiagnostics({
  diagnostics,
}: {
  readonly diagnostics: readonly MediaDiagnostic[];
}) {
  return (
    <section
      aria-labelledby="media-blocking-diagnostics-title"
      className="media-diagnostics"
    >
      <h3 id="media-blocking-diagnostics-title">阻断性诊断</h3>
      {diagnostics.length > 0 ? (
        <ul>
          {diagnostics.map((diagnostic, index) => (
            <li key={`${diagnostic.code}-${diagnostic.cueId ?? "document"}-${index}`}>
              <strong>{diagnostic.code}</strong>
              <span>{redactLocalPaths(diagnostic.message)}</span>
              <small>
                {diagnostic.cueId ? `cue：${diagnostic.cueId}` : "文档级诊断"}
                {diagnostic.sceneId ? ` · scene：${diagnostic.sceneId}` : ""}
              </small>
            </li>
          ))}
        </ul>
      ) : (
        <p role="status">没有阻断性诊断。</p>
      )}
    </section>
  );
}

function CaptionsDocumentSummary({
  document,
}: {
  readonly document: CaptionsMediaDocument;
}) {
  const mappings = summarizeMappings(document.mappings);
  const diagnostics = summarizeDiagnostics(document.diagnostics);
  return (
    <section
      aria-labelledby="media-captions-summary-title"
      className="media-document-card media-captions-summary"
      data-testid="media-captions-summary"
    >
      <header className="media-document-heading">
        <div>
          <h2 id="media-captions-summary-title">正式字幕文档</h2>
          <p>cue、时间映射、诊断与事实引用均来自当前正式产物。</p>
        </div>
      </header>
      <dl className="media-document-metadata media-caption-stats">
        <dt>来源文件</dt>
        <dd>{sourceFileName(document.source.sourceFileName)}</dd>
        <dt>原始内容哈希</dt>
        <dd><code>{document.rawContentHash}</code></dd>
        <dt>来源大小</dt>
        <dd>{formatBytes(document.source.byteLength)}</dd>
        <dt>cue 数</dt>
        <dd>{document.cues.length}</dd>
        <dt>映射数</dt>
        <dd>{document.mappings.length}</dd>
        <dt>诊断数</dt>
        <dd>
          {document.diagnostics.length}（info {diagnostics.info} / warning {diagnostics.warning} / error {diagnostics.error}）
        </dd>
        <dt>timingPrecision</dt>
        <dd>
          <code>cue_exact</code> {mappings.cueExact} · <code>estimated</code> {mappings.estimated}
        </dd>
        <dt>timingBasis</dt>
        <dd>
          <code>srt_cue</code> {mappings.srtCue} · <code>sentence_interpolation</code> {mappings.sentenceInterpolation} · <code>word_interpolation</code> {mappings.wordInterpolation}
        </dd>
      </dl>
      <BlockingDiagnostics diagnostics={diagnostics.blocking} />
      <section aria-labelledby="media-cue-table-title" className="media-cues">
        <h3 id="media-cue-table-title">Cue 明细与追溯</h3>
        <CueTable cues={document.cues} />
      </section>
    </section>
  );
}

export function MediaDocumentSummary({
  document,
  stageId,
}: MediaDocumentSummaryProps) {
  if (!document) {
    return (
      <section
        aria-label="正式媒体文档摘要"
        className="media-document-card media-document-empty"
        data-testid="media-document-summary"
      >
        <h2>尚无正式{stageId === "audio" ? "音频" : "字幕"}文档</h2>
        <p>完成导入任务并刷新阶段后，这里将显示经过契约校验的正式产物。</p>
      </section>
    );
  }
  return document.documentType === "audio_media" ? (
    <AudioDocumentSummary document={document} />
  ) : (
    <CaptionsDocumentSummary document={document} />
  );
}
