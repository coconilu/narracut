import { Icon } from "../../components/icons";
import type {
  RecoverySummary,
  WorkbenchEvent,
  WorkbenchJob,
} from "../../lib/desktop-gateway";

export type ActivityTab = "events" | "logs" | "artifacts";

interface ActivityPanelProps {
  readonly activeTab: ActivityTab;
  readonly jobs: readonly WorkbenchJob[];
  readonly events: readonly WorkbenchEvent[];
  readonly recovery: RecoverySummary;
  readonly busy: boolean;
  readonly onTabChange: (tab: ActivityTab) => void;
  readonly onRecover: () => Promise<void>;
}

const tabLabels: Record<ActivityTab, string> = {
  events: "事件",
  logs: "日志",
  artifacts: "产物",
};

export function ActivityPanel({
  activeTab,
  jobs,
  events,
  recovery,
  busy,
  onTabChange,
  onRecover,
}: ActivityPanelProps) {
  const runningCount = jobs.filter((job) => ["running", "retrying", "queued"].includes(job.status)).length;
  const warningCount = events.filter((event) => event.tone === "warning").length + recovery.warnings;
  const indexNeedsRebuild =
    recovery.warnings > 0 || jobs.some((job) => !job.indexSynchronized);
  const artifacts = uniqueArtifacts(jobs, events);

  return (
    <section className="activity" aria-label="任务活动">
      <div className="activity-toolbar">
        <div className="activity-tabs" role="tablist" aria-label="活动类型">
          {(Object.keys(tabLabels) as ActivityTab[]).map((tab) => (
            <button
              aria-selected={activeTab === tab}
              className={`activity-tab ${activeTab === tab ? "active" : ""}`}
              data-testid={`activity-${tab}`}
              key={tab}
              onClick={() => onTabChange(tab)}
              role="tab"
              type="button"
            >
              {tabLabels[tab]}
            </button>
          ))}
        </div>
        <div className="activity-summary">
          <span>
            {runningCount} 个运行中 · {warningCount} 个警告 · {indexNeedsRebuild ? "索引需重建" : "索引已同步"}
          </span>
          <button
            className="activity-refresh"
            disabled={busy}
            onClick={() => void onRecover()}
            title="恢复中断任务并重建任务索引"
            type="button"
          >
            <Icon name="refresh" size={14} />
            恢复任务
          </button>
        </div>
      </div>

      {activeTab === "events" ? <EventRows events={events} /> : null}
      {activeTab === "logs" ? <LogRows events={events} /> : null}
      {activeTab === "artifacts" ? <ArtifactRows artifacts={artifacts} /> : null}
    </section>
  );
}

function EventRows({ events }: { readonly events: readonly WorkbenchEvent[] }) {
  if (events.length === 0) return <ActivityEmpty text="当前工程还没有任务事件。" />;
  return (
    <div className="event-table" role="tabpanel">
      {events.slice(0, 5).map((event) => (
        <div className="event-row" key={event.eventId}>
          <span className="event-time">{formatEventTime(event.createdAt)}</span>
          <span className="event-kind"><i className={`state-dot ${event.tone}`} />{event.kind}</span>
          <span className="event-message">{event.message}</span>
          <span className="event-detail">
            {event.progress !== undefined ? (
              <span className="progress-track"><span style={{ width: `${Math.round(event.progress * 100)}%` }} /></span>
            ) : event.artifactId ? event.artifactId : null}
          </span>
          <span className="event-action">
            {event.progress !== undefined
              ? `${Math.round(event.progress * 100)}%`
              : event.artifactId?.startsWith("artifact_")
                ? "查看产物"
                : event.artifactId?.startsWith("claim_")
                  ? "定位"
                  : "详情"}
          </span>
        </div>
      ))}
    </div>
  );
}

function LogRows({ events }: { readonly events: readonly WorkbenchEvent[] }) {
  if (events.length === 0) return <ActivityEmpty text="当前运行尚未写入日志。" />;
  return (
    <div className="log-list" role="tabpanel">
      {events.slice(0, 6).map((event) => (
        <div key={event.eventId}><span>{formatEventTime(event.createdAt)}</span><code>{event.kind}</code><p>{event.message}</p></div>
      ))}
    </div>
  );
}

function ArtifactRows({ artifacts }: { readonly artifacts: readonly string[] }) {
  if (artifacts.length === 0) return <ActivityEmpty text="当前运行还没有可查看产物。" />;
  return (
    <div className="artifact-list" role="tabpanel">
      {artifacts.map((artifactId) => (
        <div key={artifactId}><span className="artifact-icon" aria-hidden="true" /><strong>{artifactId}</strong><span>当前运行 · 内容寻址</span></div>
      ))}
    </div>
  );
}

function ActivityEmpty({ text }: { readonly text: string }) {
  return <div className="activity-empty" role="tabpanel">{text}</div>;
}

function uniqueArtifacts(
  jobs: readonly WorkbenchJob[],
  events: readonly WorkbenchEvent[],
): readonly string[] {
  const ids = new Set<string>();
  for (const job of jobs) for (const id of job.artifactIds) ids.add(id);
  for (const event of events) if (event.artifactId?.startsWith("artifact_")) ids.add(event.artifactId);
  return [...ids];
}

function formatEventTime(value: string): string {
  const date = new Date(value);
  const milliseconds = String(date.getMilliseconds()).padStart(3, "0");
  return `${date.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false })}.${milliseconds}`;
}
