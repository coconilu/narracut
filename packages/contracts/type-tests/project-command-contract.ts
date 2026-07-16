import type {
  CopyProjectRequest,
  CreateProjectRequest,
  ProjectCommandError,
  ProjectCopyResult,
  ProjectInspection,
} from "../src/index";

declare const create: CreateProjectRequest;
declare const copy: CopyProjectRequest;
declare const inspection: ProjectInspection;
declare const copyResult: ProjectCopyResult;
declare const error: ProjectCommandError;

const createCommand: "create_project" = create.command;
const copyCommand: "copy_project" = copy.command;
const historyPolicy: "preserve_immutable_source_identity" =
  copyResult.historyPolicy;

if (inspection.migration.status === "required") {
  const firstStep: string = inspection.migration.steps[0];
  void firstStep;
}

if (error.code === "migration_conflict") {
  const expected: number | undefined = error.expectedVersion;
  void expected;
}

// @ts-expect-error Project command requests are immutable.
create.name = "changed";

void createCommand;
void copyCommand;
void historyPolicy;
