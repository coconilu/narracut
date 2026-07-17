import { useEffect, useMemo, useRef, useState } from "react";
import type { ProjectDescriptor, ProviderCredentialStatus } from "@narracut/contracts";
import { Icon } from "../../components/icons";
import { isProviderCommandError } from "../../lib/provider-commands";
import {
  providerGateway,
  type ProviderSetup,
} from "../../lib/provider-gateway";

interface ProviderPanelProps {
  readonly project: ProjectDescriptor;
  readonly selectedStageId: string;
  readonly disabled: boolean;
  readonly onClose: () => void;
  readonly onQueued: () => Promise<void>;
}

interface StableScriptIntent {
  readonly signature: string;
  readonly runId: string;
  readonly idempotencyKey: string;
}

type ProviderNoCredentialStatus = Extract<ProviderCredentialStatus, { readonly storage: "none" }>;

function portableId(prefix: string): string {
  return `${prefix}${crypto.randomUUID().replace(/-/g, "").slice(0, 20)}`;
}

function describeError(reason: unknown): string {
  if (isProviderCommandError(reason)) return reason.message;
  if (reason instanceof Error) return reason.message;
  if (typeof reason === "string") return reason;
  return "Provider 操作失败，请查看任务诊断后重试。";
}

function localStatusGuidance(status: ProviderNoCredentialStatus | null): string {
  switch (status?.diagnosticCode) {
    case "ready":
      return "本机 CLI 已通过安装、登录、版本和固定能力探测。";
    case "not_installed":
      return "请先安装 Codex CLI，再返回这里重新探测。";
    case "not_logged_in":
      return "请在终端完成 codex login；NarraCut 不读取或复制登录令牌。";
    case "unsupported_version":
      return "请安装兼容的 Codex CLI 0.144.x，再重新探测。";
    case "probe_failed":
      return "固定能力探测未完成；请检查诊断并确认 CLI 可在本机直接运行。";
    default:
      return "正在等待本机 CLI 状态。";
  }
}

export function ProviderPanel({
  project,
  selectedStageId,
  disabled,
  onClose,
  onQueued,
}: ProviderPanelProps) {
  const [setup, setSetup] = useState<ProviderSetup | null>(null);
  const [providerId, setProviderId] = useState("");
  const [model, setModel] = useState("");
  const [secret, setSecret] = useState("");
  const [language, setLanguage] = useState("zh-CN");
  const [maxOutputTokens, setMaxOutputTokens] = useState(4096);
  const [busyLabel, setBusyLabel] = useState<string | null>("正在读取 Provider 状态…");
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const scriptIntentRef = useRef<StableScriptIntent | null>(null);

  useEffect(() => {
    let active = true;
    setBusyLabel("正在读取 Provider 状态…");
    providerGateway
      .loadSetup()
      .then((nextSetup) => {
        if (!active) return;
        setSetup(nextSetup);
        const first = nextSetup.providers[0];
        if (first) {
          setProviderId(first.providerId);
          setModel(first.defaultModel);
        }
      })
      .catch((reason) => {
        if (active) setError(describeError(reason));
      })
      .finally(() => {
        if (active) setBusyLabel(null);
      });
    return () => {
      active = false;
    };
  }, []);

  const provider = useMemo(
    () => setup?.providers.find((candidate) => candidate.providerId === providerId),
    [providerId, setup?.providers],
  );
  const emptyCatalog = setup !== null && setup.providers.length === 0;
  const credentialStatus = setup?.credentials[providerId];
  const localStatus = credentialStatus?.storage === "none" ? credentialStatus : null;
  const usesLocalCli = provider?.credentialStorage === "none";
  const configured = credentialStatus?.configured ?? false;
  const selectedModel = provider?.models.find((candidate) => candidate.modelId === model);
  const blocked = disabled || busyLabel !== null;

  async function saveCredential() {
    if (!provider || blocked) return;
    if (secret.length < 20) {
      setError("API Key 至少需要 20 个字符。");
      return;
    }
    setBusyLabel("正在写入系统 Keyring…");
    setError(null);
    setNotice(null);
    try {
      setSetup(await providerGateway.setCredential(provider.providerId, secret));
      setSecret("");
      setNotice("凭据已保存到系统 Keyring；项目文件、SQLite 与日志不会保存密钥。");
    } catch (reason) {
      setError(describeError(reason));
    } finally {
      setBusyLabel(null);
    }
  }

  async function deleteCredential() {
    if (!provider || blocked) return;
    setBusyLabel("正在删除系统凭据…");
    setError(null);
    setNotice(null);
    try {
      setSetup(await providerGateway.deleteCredential(provider.providerId));
      setSecret("");
      setNotice("系统凭据已删除。历史任务与 Artifact 仍保留，但不包含密钥。");
    } catch (reason) {
      setError(describeError(reason));
    } finally {
      setBusyLabel(null);
    }
  }

  async function refreshProviderStatus() {
    if (blocked) return;
    setBusyLabel("正在重新探测 Provider 状态…");
    setError(null);
    setNotice(null);
    try {
      const nextSetup = await providerGateway.loadSetup();
      setSetup(nextSetup);
      const nextProvider =
        nextSetup.providers.find((candidate) => candidate.providerId === providerId) ??
        nextSetup.providers[0];
      if (nextProvider && nextProvider.providerId !== providerId) {
        setProviderId(nextProvider.providerId);
        setModel(nextProvider.defaultModel);
      }
    } catch (reason) {
      setError(describeError(reason));
    } finally {
      setBusyLabel(null);
    }
  }

  async function enqueueScript() {
    if (!provider || !selectedModel || blocked) return;
    if (!configured) {
      setError(localStatus?.diagnostic ?? "请先配置系统凭据，再发起结构化脚本任务。");
      return;
    }
    const signature = JSON.stringify({
      projectId: project.projectId,
      providerId: provider.providerId,
      model: selectedModel.modelId,
      language,
      maxOutputTokens,
    });
    if (scriptIntentRef.current?.signature !== signature) {
      scriptIntentRef.current = {
        signature,
        runId: portableId("run_script_api_"),
        idempotencyKey: portableId("idem_script_api_"),
      };
    }
    const intent = scriptIntentRef.current;
    setBusyLabel("正在冻结输入并创建脚本任务…");
    setError(null);
    setNotice(null);
    try {
      const result = await providerGateway.enqueueScript({
        projectPath: project.projectPath,
        expectedProjectId: project.projectId,
        providerId: provider.providerId,
        model: selectedModel.modelId,
        runId: intent.runId,
        idempotencyKey: intent.idempotencyKey,
        language,
        maxOutputTokens,
      });
      scriptIntentRef.current = null;
      setNotice(`任务 ${result.jobId} 已入队；运行 ${result.runId} 会保留输入与配置快照。`);
      await onQueued();
    } catch (reason) {
      setError(describeError(reason));
    } finally {
      setBusyLabel(null);
    }
  }

  return (
    <aside aria-label="AI Provider 配置与脚本运行" className="provider-panel" data-testid="provider-panel">
      <header className="provider-panel-header">
        <div>
          <span className="eyebrow">AI PROVIDER · V1</span>
          <h2>结构化生成</h2>
          <p>远程 API 与本机 Codex CLI 共用同一条可追溯、可取消边界。</p>
        </div>
        <button aria-label="关闭 Provider 面板" disabled={blocked} onClick={onClose} type="button">
          <Icon name="x" size={15} />
        </button>
      </header>

      <div className="provider-panel-body">
        <section className="provider-card">
          <div className="provider-card-title">
            <div><span>执行器</span><strong>{provider?.displayName ?? (emptyCatalog ? "无可用 Provider" : "正在加载")}</strong></div>
            <span className={`provider-status ${configured ? "configured" : "missing"}`}>
              {configured ? (usesLocalCli ? "CLI 就绪" : "已配置") : emptyCatalog ? "不可用" : usesLocalCli ? "CLI 未就绪" : "缺少凭据"}
            </span>
          </div>
          <label className="provider-field">
            <span>Provider</span>
            <select
              disabled={blocked || !setup?.providers.length}
              onChange={(event) => {
                const nextId = event.target.value;
                const nextProvider = setup?.providers.find((item) => item.providerId === nextId);
                setProviderId(nextId);
                setModel(nextProvider?.defaultModel ?? "");
                setError(null);
                setNotice(null);
              }}
              value={providerId}
            >
              {setup?.providers.map((item) => (
                <option key={item.providerId} value={item.providerId}>{item.displayName}</option>
              ))}
            </select>
          </label>
          <label className="provider-field">
            <span>模型</span>
            <select disabled={blocked || !provider} onChange={(event) => setModel(event.target.value)} value={model}>
              {provider?.models.map((item) => (
                <option key={item.modelId} value={item.modelId}>{item.displayName}</option>
              ))}
            </select>
          </label>
          <div className="provider-capabilities" aria-label="Provider 能力">
            <span>严格 JSON Schema</span><span>可取消</span><span>用量诊断</span>
          </div>
        </section>

        {usesLocalCli ? (
          <section className="provider-card">
            <div className="provider-card-title">
              <div><span>本机运行时</span><strong>Codex CLI 只读执行舱</strong></div>
            </div>
            <div className="provider-diagnostic-grid" aria-label="Codex CLI 状态">
              <div><span>安装</span><strong className={localStatus?.installed ? "ready" : "missing"}>{localStatus?.installed ? "已发现" : "未发现"}</strong></div>
              <div><span>登录</span><strong className={localStatus?.loggedIn ? "ready" : "missing"}>{localStatus?.loggedIn ? "已登录" : "未登录"}</strong></div>
              <div><span>版本</span><strong className={localStatus?.versionSupported ? "ready" : "missing"}>{localStatus?.cliVersion ?? "未知"}</strong></div>
            </div>
            <p className="provider-help">{localStatusGuidance(localStatus)}</p>
            {localStatus?.diagnostic ? (
              <div className="provider-diagnostic" data-diagnostic-code={localStatus.diagnosticCode}>
                {localStatus.diagnostic}
              </div>
            ) : null}
            <p className="provider-help">不接收 API Key，不读取或复制 Codex 登录令牌；运行时重新校验 CLI 版本与可执行文件哈希。</p>
            <div className="provider-actions">
              <button className="button" disabled={blocked} onClick={() => void refreshProviderStatus()} type="button">重新探测</button>
            </div>
          </section>
        ) : (
          <section className="provider-card">
            <div className="provider-card-title"><div><span>凭据存储</span><strong>系统 Keyring</strong></div></div>
            <label className="provider-field">
              <span>API Key</span>
              <input
                autoComplete="off"
                disabled={blocked}
                onChange={(event) => setSecret(event.target.value)}
                placeholder={configured ? "输入新密钥以替换" : "仅写入系统凭据库"}
                spellCheck={false}
                type="password"
                value={secret}
              />
            </label>
            <p className="provider-help">界面只显示“是否配置”，不会读取、回显或写入工程。</p>
            <div className="provider-actions">
              <button className="button" disabled={blocked || !configured} onClick={() => void deleteCredential()} type="button">删除凭据</button>
              <button className="button primary" disabled={blocked || secret.length < 20} onClick={() => void saveCredential()} type="button">保存到 Keyring</button>
            </div>
          </section>
        )}

        <section className="provider-card provider-script-card">
          <div className="provider-card-title">
            <div><span>阶段运行</span><strong>Brief / Research → Script</strong></div>
            <span className="provider-stage-mark">SCRIPT</span>
          </div>
          <div className="provider-grid">
            <label className="provider-field"><span>语言</span><input disabled={blocked} maxLength={35} onChange={(event) => setLanguage(event.target.value)} value={language} /></label>
            <label className="provider-field">
              <span>最大输出</span>
              <input
                disabled={blocked}
                max={Math.min(selectedModel?.maxOutputTokens ?? 32768, 32768)}
                min={256}
                onChange={(event) => setMaxOutputTokens(Number(event.target.value))}
                step={256}
                type="number"
                value={maxOutputTokens}
              />
            </label>
          </div>
          <p className="provider-help">只解析当前有效的已审核产物；输出 claimId / evidenceRef 必须是输入引用子集。</p>
          <button
            className="button primary provider-run-button"
            data-testid="enqueue-script-provider"
            disabled={
              blocked || selectedStageId !== "script" || !configured || !selectedModel ||
              !Number.isInteger(maxOutputTokens) || language.trim().length < 2 ||
              maxOutputTokens < 256 ||
              maxOutputTokens > Math.min(selectedModel?.maxOutputTokens ?? 32768, 32768)
            }
            onClick={() => void enqueueScript()}
            type="button"
          >
            {selectedStageId === "script" ? "创建结构化脚本任务" : "请先选择 Script 阶段"}
          </button>
        </section>

        {notice ? <div className="provider-message success" role="status">{notice}</div> : null}
        {error ? <div className="provider-message error" role="alert">{error}</div> : null}
      </div>

      <footer className="provider-panel-footer">
        <span>{providerGateway.mode === "demo" ? "演示状态" : "本机状态"}</span>
        <strong>{busyLabel ?? "Provider 边界就绪"}</strong>
      </footer>
    </aside>
  );
}
