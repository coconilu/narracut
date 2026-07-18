import assert from "node:assert/strict";
import test from "node:test";
import {
  isMediaCommandError,
  mediaCommandErrorCodes,
  mediaOperations,
} from "./media-commands-model.js";

test("错误守卫接受三个同步命令的全部错误码基础形状", () => {
  for (const code of mediaCommandErrorCodes) {
    for (const operation of mediaOperations) {
      assert.equal(
        isMediaCommandError({
          apiVersion: "1.0.0",
          code,
          operation,
          message: "安全错误摘要",
          retryable: false,
        }),
        true,
      );
    }
  }
});

test("错误守卫拒绝未知枚举、错误字段类型与意外路径字段", () => {
  const valid = {
    apiVersion: "1.0.0",
    code: "invalid_request",
    operation: "get_media_document",
    message: "安全错误摘要",
    retryable: false,
  };
  for (const invalid of [
    { ...valid, code: "unknown" },
    { ...valid, operation: "unknown" },
    { ...valid, message: 42 },
    { ...valid, retryable: "false" },
    { ...valid, diagnosticIds: "diagnostic_001" },
    { ...valid, path: "C:/Users/private/project" },
  ]) {
    assert.equal(isMediaCommandError(invalid), false);
  }
});
