import type { ArtifactRead } from "./storage-commands";
import type { StageRun } from "@narracut/contracts";
import type { WorkbenchJob } from "./desktop-gateway";

export function artifactReadMatchesRun(
  projectId: string,
  run: StageRun,
  read: ArtifactRead,
): boolean;

export function findJobByRunId(
  jobs: readonly WorkbenchJob[],
  runId: string,
): WorkbenchJob | undefined;
