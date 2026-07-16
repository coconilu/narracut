import type { Artifact as ArtifactContract } from "./generated/contracts-v1";

export const NARRACUT_CONTRACT_VERSION = "1.0.0" as const;
export const NARRACUT_PROJECT_COMMAND_API_VERSION = "1.0.0" as const;
export const NARRACUT_STORAGE_COMMAND_API_VERSION = "1.0.0" as const;

export type * from "./generated/contracts-v1";
export type * from "./generated/project-commands-v1";
export type * from "./generated/storage-commands-v1";

type ArtifactDraftSource<T extends ArtifactContract> = T["source"] extends {
  readonly origin: "imported";
}
  ? Omit<T["source"], "sourceContentHash">
  : T["source"];

type ToArtifactDraft<T> = T extends ArtifactContract
  ? Pick<
      T,
      | "stageId"
      | "runId"
      | "kind"
      | "mediaType"
      | "evidenceRole"
      | "provenance"
    > & {
      readonly source: ArtifactDraftSource<T>;
    }
  : never;

/**
 * Artifact Store 在计算 artifactId、URI、SHA-256、字节数和创建时间前接收的草稿。
 * 该类型从持久化 Artifact 判别联合派生，保留 source 与 evidenceRole 的关联约束。
 */
export type ArtifactDraft = ToArtifactDraft<ArtifactContract>;
