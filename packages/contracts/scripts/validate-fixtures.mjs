import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import Ajv2020 from "ajv/dist/2020.js";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const schema = await readJson("schema/narracut-contracts-v1.schema.json");
const validDocuments = await readJson("fixtures/valid-documents.json");
const invalidCases = await readJson("fixtures/invalid-documents.json");

const ajv = new Ajv2020({ allErrors: true, strict: true });
const validate = ajv.compile(schema);
let failed = false;

for (const document of validDocuments) {
  if (!validate(document)) {
    failed = true;
    console.error(
      `合法夹具 ${document.documentType ?? "unknown"} 未通过校验：`,
      validate.errors,
    );
  }
}

for (const testCase of invalidCases) {
  const source = validDocuments.find(
    (document) => document.documentType === testCase.sourceDocumentType,
  );
  if (!source) {
    failed = true;
    console.error(
      `非法夹具 ${testCase.name} 找不到来源 ${testCase.sourceDocumentType}。`,
    );
    continue;
  }

  const document = structuredClone(source);
  for (const patch of testCase.patches ?? [testCase.patch]) {
    applyPatch(document, patch);
  }

  if (validate(document)) {
    failed = true;
    console.error(`非法夹具 ${testCase.name} 被错误接受。`);
  }
}

if (failed) {
  process.exitCode = 1;
} else {
  console.log(
    `契约夹具验证通过：${validDocuments.length} 个合法文档，${invalidCases.length} 个非法文档。`,
  );
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
