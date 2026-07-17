import {
  NARRACUT_PROVIDER_API_VERSION,
  type ProviderCapability,
  type ProviderCredentialStatus,
  type ScriptStageEnqueueResult,
} from "@narracut/contracts";
import { providerCommands, type ScriptStageEnqueueInput } from "./provider-commands";

export interface ProviderSetup {
  readonly providers: readonly ProviderCapability[];
  readonly credentials: Readonly<Record<string, ProviderCredentialStatus>>;
}

interface ProviderGateway {
  readonly mode: "desktop" | "demo";
  loadSetup(): Promise<ProviderSetup>;
  setCredential(providerId: string, secret: string): Promise<ProviderSetup>;
  deleteCredential(providerId: string): Promise<ProviderSetup>;
  enqueueScript(input: ScriptStageEnqueueInput): Promise<ScriptStageEnqueueResult>;
}

const DEMO_OPENAI_PROVIDER: ProviderCapability = {
  providerId: "openai_api",
  displayName: "OpenAI API",
  transport: "remote_api",
  credentialStorage: "system_keyring",
  supportsStreaming: false,
  supportsCancellation: true,
  reportsUsage: true,
  defaultModel: "gpt-5.6-terra",
  models: [
    {
      modelId: "gpt-5.6-terra",
      displayName: "GPT-5.6 Terra",
      supportedTasks: ["script_generation"],
      structuredOutputs: true,
      maxOutputTokens: 32768,
    },
  ],
};

const DEMO_CODEX_PROVIDER: ProviderCapability = {
  providerId: "local_codex",
  displayName: "本机 Codex CLI",
  transport: "local_cli",
  credentialStorage: "none",
  supportsStreaming: false,
  supportsCancellation: true,
  reportsUsage: true,
  defaultModel: "gpt-5.6-terra",
  models: [
    {
      modelId: "gpt-5.6-terra",
      displayName: "GPT-5.6 Terra",
      supportedTasks: ["script_generation"],
      structuredOutputs: true,
      maxOutputTokens: 32768,
    },
  ],
};

let demoConfigured = false;

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function loadDesktopSetup(): Promise<ProviderSetup> {
  const catalog = await providerCommands.catalog();
  const statuses = await Promise.all(
    catalog.providers.map((provider) =>
      providerCommands.credentialStatus(provider.providerId),
    ),
  );
  return {
    providers: catalog.providers,
    credentials: Object.fromEntries(statuses.map((status) => [status.providerId, status])),
  };
}

const realGateway: ProviderGateway = {
  mode: "desktop",
  loadSetup: loadDesktopSetup,
  async setCredential(providerId, secret) {
    await providerCommands.setCredential(providerId, secret);
    return loadDesktopSetup();
  },
  async deleteCredential(providerId) {
    await providerCommands.deleteCredential(providerId);
    return loadDesktopSetup();
  },
  enqueueScript: providerCommands.enqueueScript,
};

function demoSetup(): ProviderSetup {
  return {
    providers: [DEMO_OPENAI_PROVIDER, DEMO_CODEX_PROVIDER],
    credentials: {
      openai_api: {
        apiVersion: NARRACUT_PROVIDER_API_VERSION,
        messageType: "provider_credential_status",
        providerId: "openai_api",
        configured: demoConfigured,
        storage: "system_keyring",
      },
      local_codex: {
        apiVersion: NARRACUT_PROVIDER_API_VERSION,
        messageType: "provider_credential_status",
        providerId: "local_codex",
        configured: false,
        storage: "none",
        installed: false,
        loggedIn: false,
        versionSupported: false,
        diagnosticCode: "probe_failed",
        diagnostic: "浏览器演示模式不能探测本机 Codex CLI；请在 Tauri 桌面端查看真实状态。",
      },
    },
  };
}

const demoGateway: ProviderGateway = {
  mode: "demo",
  async loadSetup() {
    return demoSetup();
  },
  async setCredential(providerId, secret) {
    if (providerId !== "openai_api") throw new Error("本机 Codex CLI 不接收或保存凭据。");
    if (secret.length < 20) throw new Error("演示 API Key 至少需要 20 个字符。");
    demoConfigured = true;
    return demoSetup();
  },
  async deleteCredential(providerId) {
    if (providerId !== "openai_api") throw new Error("本机 Codex CLI 没有可删除的应用凭据。");
    demoConfigured = false;
    return demoSetup();
  },
  async enqueueScript(input) {
    const status = demoSetup().credentials[input.providerId];
    if (!status?.configured) {
      if (status?.storage === "none") throw new Error(status.diagnostic);
      throw new Error("请先把 API Key 保存到系统凭据库。");
    }
    return {
      apiVersion: NARRACUT_PROVIDER_API_VERSION,
      messageType: "script_stage_enqueue_result",
      ownerProjectId: input.expectedProjectId,
      providerRequestId: `provider_request_${input.idempotencyKey}`,
      jobId: `job_demo_${input.idempotencyKey}`,
      runId: input.runId,
      status: "queued",
    };
  },
};

export const providerGateway: ProviderGateway = isTauriRuntime()
  ? realGateway
  : demoGateway;
