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

if (failed) {
  process.exitCode = 1;
} else {
  console.log(
    [
      `持久化契约：${validDocuments.length} 个合法文档 / ${invalidDocumentCases.length} 个非法文档`,
      `项目命令契约：${validCommandMessages.length} 个合法消息 / ${invalidCommandCases.length} 个非法消息`,
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
