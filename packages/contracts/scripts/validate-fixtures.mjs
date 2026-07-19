import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import Ajv2020 from "ajv/dist/2020.js";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ajv = new Ajv2020({ allErrors: true, strict: true });

const documentSchema = await readJson(
  "schema/narracut-contracts-v1.schema.json",
);
const validDocuments = await readJson("fixtures/valid-documents.json");
const invalidDocumentCases = await readJson("fixtures/invalid-documents.json");

const commandSchema = await readJson(
  "schema/narracut-project-commands-v1.schema.json",
);
const validCommandMessages = await readJson(
  "fixtures/valid-project-command-messages.json",
);
const invalidCommandCases = await readJson(
  "fixtures/invalid-project-command-messages.json",
);
const storageCommandSchema = await readJson(
  "schema/narracut-storage-commands-v1.schema.json",
);
const validStorageCommandMessages = await readJson(
  "fixtures/valid-storage-command-messages.json",
);
const invalidStorageCommandCases = await readJson(
  "fixtures/invalid-storage-command-messages.json",
);
const workflowCommandSchema = await readJson(
  "schema/narracut-workflow-commands-v1.schema.json",
);
const validWorkflowCommandMessages = await readJson(
  "fixtures/valid-workflow-command-messages.json",
);
const invalidWorkflowCommandCases = await readJson(
  "fixtures/invalid-workflow-command-messages.json",
);
const jobCommandSchema = await readJson(
  "schema/narracut-job-commands-v1.schema.json",
);
const validJobCommandMessages = await readJson(
  "fixtures/valid-job-command-messages.json",
);
const invalidJobCommandCases = await readJson(
  "fixtures/invalid-job-command-messages.json",
);
const mediaSchema = await readJson("schema/narracut-media-v1.schema.json");
const mediaLegacySchema = await readJson(
  "schema/narracut-media-v1.0.schema.json",
);
const validMediaDocuments = await readJson("fixtures/valid-media-documents.json");
const invalidMediaDocumentCases = await readJson(
  "fixtures/invalid-media-documents.json",
);
const mediaCommandSchema = await readJson(
  "schema/narracut-media-commands-v1.schema.json",
);
const validMediaCommandMessages = await readJson(
  "fixtures/valid-media-command-messages.json",
);
const invalidMediaCommandCases = await readJson(
  "fixtures/invalid-media-command-messages.json",
);
const providerSchema = await readJson(
  "schema/narracut-provider-v1.schema.json",
);
const validProviderMessages = await readJson(
  "fixtures/valid-provider-messages.json",
);
const invalidProviderCases = await readJson(
  "fixtures/invalid-provider-messages.json",
);
const rendererSchema = await readJson(
  "schema/narracut-renderer-v1.schema.json",
);
const validRendererMessages = await readJson(
  "fixtures/valid-renderer-messages.json",
);
const invalidRendererCases = await readJson(
  "fixtures/invalid-renderer-messages.json",
);
const exportSchema = await readJson("schema/narracut-export-v1.schema.json");
const validExportMessages = await readJson("fixtures/valid-export-messages.json");
const invalidExportCases = await readJson("fixtures/invalid-export-messages.json");

let failed = false;

validateDocumentFixtures(
  ajv.compile(documentSchema),
  validDocuments,
  invalidDocumentCases,
);
validateIndexedFixtures(
  ajv.compile(commandSchema),
  validCommandMessages,
  invalidCommandCases,
  "project-command",
);
validateIndexedFixtures(
  ajv.compile(storageCommandSchema),
  validStorageCommandMessages,
  invalidStorageCommandCases,
  "storage-command",
);
validateIndexedFixtures(
  ajv.compile(workflowCommandSchema),
  validWorkflowCommandMessages,
  invalidWorkflowCommandCases,
  "workflow-command",
);
validateIndexedFixtures(
  ajv.compile(jobCommandSchema),
  validJobCommandMessages,
  invalidJobCommandCases,
  "job-command",
);
const validateCurrentMediaDocument = ajv.compile(mediaSchema);
const validateLegacyMediaDocument = ajv.compile(mediaLegacySchema);
for (const documentType of ["captions_media", "scene_plan"]) {
  const source = validMediaDocuments.find(
    (document) => document.documentType === documentType,
  );
  const mislabeledLegacy = structuredClone(source);
  mislabeledLegacy.schemaVersion = "1.0.0";
  if (validateCurrentMediaDocument(mislabeledLegacy)) {
    failed = true;
    console.error(
      `公开 media Schema 错误接受了携带 1.1 字段的 1.0 ${documentType} 文档。`,
    );
  }
}
function validateMediaDocument(document) {
  const validator =
    document?.schemaVersion === "1.0.0"
      ? validateLegacyMediaDocument
      : validateCurrentMediaDocument;
  const valid = validator(document);
  validateMediaDocument.errors = validator.errors;
  return valid;
}
validateDocumentFixtures(
  validateMediaDocument,
  validMediaDocuments,
  invalidMediaDocumentCases,
);
validateIndexedFixtures(
  ajv.compile(mediaCommandSchema),
  validMediaCommandMessages,
  invalidMediaCommandCases,
  "media-command",
);
validateIndexedFixtures(
  ajv.compile(providerSchema),
  validProviderMessages,
  invalidProviderCases,
  "provider",
);
validateIndexedFixtures(
  ajv.compile(rendererSchema),
  validRendererMessages,
  invalidRendererCases,
  "renderer",
);
validateIndexedFixtures(
  ajv.compile(exportSchema),
  validExportMessages,
  invalidExportCases,
  "export",
);

if (failed) {
  process.exitCode = 1;
} else {
  console.log(
    [
      `持久化契约：${validDocuments.length} 个合法文档 / ${invalidDocumentCases.length} 个非法文档`,
      `项目命令契约：${validCommandMessages.length} 个合法消息 / ${invalidCommandCases.length} 个非法消息`,
      `存储命令契约：${validStorageCommandMessages.length} 个合法消息 / ${invalidStorageCommandCases.length} 个非法消息`,
      `工作流命令契约：${validWorkflowCommandMessages.length} 个合法消息 / ${invalidWorkflowCommandCases.length} 个非法消息`,
      `任务命令契约：${validJobCommandMessages.length} 个合法消息 / ${invalidJobCommandCases.length} 个非法消息`,
      `媒体契约：${validMediaDocuments.length} 个合法文档 / ${invalidMediaDocumentCases.length} 个非法文档`,
      `媒体命令契约：${validMediaCommandMessages.length} 个合法消息 / ${invalidMediaCommandCases.length} 个非法消息`,
      `Provider 契约：${validProviderMessages.length} 个合法消息 / ${invalidProviderCases.length} 个非法消息`,
      `Renderer 契约：${validRendererMessages.length} 个合法消息 / ${invalidRendererCases.length} 个非法消息`,
      `Export 契约：${validExportMessages.length} 个合法消息 / ${invalidExportCases.length} 个非法消息`,
    ].join("；"),
  );
}

function validateDocumentFixtures(validate, validFixtures, invalidCases) {
  for (const document of validFixtures) {
    if (!validate(document)) {
      failed = true;
      console.error(
        `合法夹具 ${document.documentType ?? "unknown"} 未通过校验：`,
        validate.errors,
      );
    }
  }

  for (const testCase of invalidCases) {
    const source = validFixtures.find(
      (document) => document.documentType === testCase.sourceDocumentType,
    );
    if (!source) {
      failed = true;
      console.error(
        `非法夹具 ${testCase.name} 找不到来源 ${testCase.sourceDocumentType}。`,
      );
      continue;
    }

    validatePatchedFixture(validate, source, testCase);
  }
}

function validateIndexedFixtures(validate, validFixtures, invalidCases, label) {
  for (const [index, message] of validFixtures.entries()) {
    if (!validate(message)) {
      failed = true;
      console.error(
        `合法 ${label} 夹具 #${index} 未通过校验：`,
        validate.errors,
      );
    }
  }

  for (const testCase of invalidCases) {
    const source = validFixtures[testCase.sourceIndex];
    if (!source) {
      failed = true;
      console.error(
        `非法夹具 ${testCase.name} 找不到索引 ${testCase.sourceIndex}。`,
      );
      continue;
    }

    validatePatchedFixture(validate, source, testCase);
  }
}

function validatePatchedFixture(validate, source, testCase) {
  const document = structuredClone(source);
  for (const patch of testCase.patches ?? [testCase.patch]) {
    applyPatch(document, patch);
  }

  if (validate(document)) {
    failed = true;
    console.error(`非法夹具 ${testCase.name} 被错误接受。`);
  }
}

async function readJson(relativePath) {
  return JSON.parse(await readFile(resolve(packageRoot, relativePath), "utf8"));
}

function applyPatch(document, patch) {
  const parent = patch.path
    .slice(0, -1)
    .reduce((value, segment) => value[segment], document);
  const key = patch.path.at(-1);

  if (patch.op === "remove") {
    delete parent[key];
    return;
  }

  if (patch.op === "replace") {
    parent[key] = patch.value;
    return;
  }

  throw new Error(`不支持的夹具 patch 操作：${patch.op}`);
}
