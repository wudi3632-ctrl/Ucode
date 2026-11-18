import assert from "node:assert/strict";
import test from "node:test";
import { readJsonBody } from "../agent/runtime/body.js";

async function* splitInsideUtf8(value, needle) {
  const body = Buffer.from(JSON.stringify(value), "utf8");
  const marker = Buffer.from(needle, "utf8");
  const markerStart = body.indexOf(marker);
  assert.notEqual(markerStart, -1);
  const split = markerStart + 1;
  yield body.subarray(0, split);
  yield body.subarray(split, split + 1);
  yield body.subarray(split + 1);
}

test("JSON request bodies preserve Chinese text across UTF-8 byte boundaries", async () => {
  const input = {
    title: "存在未运行的自动服务，需要结合具体症状核对",
    summary: "只依据真实系统证据进行只读诊断",
  };
  const parsed = await readJsonBody(splitInsideUtf8(input, "核"));
  assert.deepEqual(parsed, input);
  assert.equal(JSON.stringify(parsed).includes("�"), false);
});

test("JSON request body size is enforced on raw bytes", async () => {
  async function* chunks() { yield Buffer.from("12345"); }
  await assert.rejects(() => readJsonBody(chunks(), { maxBytes: 4 }), (error) => error.code === "body_too_large" && error.status === 413);
});
