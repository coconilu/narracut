import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { compileFromFile } from "json-schema-to-typescript";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const checkOnly = process.argv.includes("--check");
const targets = [
  {
    schema: "schema/narracut-contracts-v1.schema.json",
    output: "src/generated/contracts-v1.ts",
  },
  {
    schema: "schema/narracut-project-commands-v1.schema.json",
    output: "src/generated/project-commands-v1.ts",
  },
  {
    schema: "schema/narracut-storage-commands-v1.schema.json",
    output: "src/generated/storage-commands-v1.ts",
  },
];

for (const target of targets) {
  const schemaPath = resolve(packageRoot, target.schema);
  const outputPath = resolve(packageRoot, target.output);
  const generated = await compileFromFile(schemaPath, {
    bannerComment: `/* eslint-disable */
/**
 * 此文件由 ${target.schema} 自动生成。
 * 请勿手工修改；运行 pnpm --filter @narracut/contracts generate 重新生成。
 */`,
    enableConstEnums: false,
    strictIndexSignatures: true,
    unreachableDefinitions: true,
    style: {
      bracketSpacing: true,
      printWidth: 100,
      semi: true,
      singleQuote: false,
      tabWidth: 2,
      trailingComma: "all",
      useTabs: false,
    },
  });
  const expected = makeInterfacePropertiesReadonly(generated);

  if (checkOnly) {
    let current;
    try {
      current = await readFile(outputPath, "utf8");
    } catch {
      current = "";
    }

    if (current !== expected) {
      console.error(
        `生成的 TypeScript 契约 ${target.output} 已过期，请运行 pnpm --filter @narracut/contracts generate。`,
      );
      process.exitCode = 1;
    }
  } else {
    await mkdir(dirname(outputPath), { recursive: true });
    await writeFile(outputPath, expected, "utf8");
  }
}

function makeInterfacePropertiesReadonly(source) {
  const lines = source.split("\n");
  let insideInterface = false;

  return lines
    .map((line) => {
      if (/^export interface\s+\w+\s*\{$/.test(line)) {
        insideInterface = true;
        return line;
      }

      if (insideInterface && line === "}") {
        insideInterface = false;
        return line;
      }

      if (
        insideInterface &&
        /^  (?:[A-Za-z_$][\w$]*|"[^"]+")\??:/.test(line)
      ) {
        return line
          .replace(/^  /, "  readonly ")
          .replace(/: ([A-Za-z_$][\w$]*)\[\];$/, ": readonly $1[];")
          .replace(/: \[/, ": readonly [");
      }

      if (insideInterface && /^  \[.+\]:/.test(line)) {
        return line.replace(/^  /, "  readonly ");
      }

      return line;
    })
    .join("\n");
}
