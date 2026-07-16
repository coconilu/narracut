import type {
  Artifact,
  ArtifactCommitResult,
  ArtifactDraft,
  ImportArtifactFileRequest,
} from "../src/index";

const generatedDraft = {
  stageId: "script",
  runId: "run_script_001",
  kind: "script",
  mediaType: "text/markdown",
  evidenceRole: "non_evidence",
  source: {
    origin: "generated",
    providerId: "openai",
  },
  provenance: [],
} as const satisfies ArtifactDraft;

const importedDraft = {
  stageId: "research",
  runId: "run_research_001",
  kind: "source_image",
  mediaType: "image/png",
  evidenceRole: "factual_evidence",
  source: {
    origin: "imported",
    sourceUri: "https://example.com/source.png",
    author: "Example Author",
    license: "CC-BY-4.0",
    attributionText: "Example Author / CC-BY-4.0",
    authorizationRecordIds: ["authorization_001"],
  },
  provenance: [
    {
      claimId: "claim_001",
      evidenceRef: "evidence_001",
    },
  ],
} as const satisfies ArtifactDraft;

const request = {
  apiVersion: "1.0.0",
  command: "import_artifact_file",
  projectPath: "C:/Videos/demo",
  expectedProjectId: "project_demo",
  sourcePath: "C:/Imports/script.md",
  artifact: generatedDraft,
} as const satisfies ImportArtifactFileRequest;

const artifact: Artifact = {
  schemaVersion: "1.0.0",
  documentType: "artifact",
  artifactId: "artifact_demo",
  projectId: "project_demo",
  stageId: "script",
  runId: "run_script_001",
  kind: "script",
  uri: "artifacts/objects/sha256/00/hash",
  contentHash: "sha256:hash",
  byteLength: 1,
  evidenceRole: "non_evidence",
  source: {
    origin: "generated",
    providerId: "openai",
  },
  provenance: [],
  createdAt: "2026-07-16T08:20:00Z",
};

const result = {
  apiVersion: "1.0.0",
  ownerProjectId: "project_demo",
  artifact: { ...artifact },
  metadataUri: "artifacts/metadata/artifact_demo.json",
  contentUri: artifact.uri,
  deduplicated: false,
  indexStatus: "up_to_date",
} as const satisfies ArtifactCommitResult;

declare function acceptDraft(value: ArtifactDraft): void;

// @ts-expect-error 生成素材不能作为事实证据
acceptDraft({ ...generatedDraft, evidenceRole: "factual_evidence" });

void request;
void result;
void importedDraft;
