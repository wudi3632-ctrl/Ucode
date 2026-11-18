import assert from "node:assert/strict";
import http from "node:http";
import test from "node:test";
import { ModelClient } from "../agent/model/client.js";

async function withServer(handler, run) {
  const server = http.createServer(handler);
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const address = server.address();
    await run(address.port);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
}

test("streaming model client preserves ordered deltas and final result", async () => {
  await withServer((_request, response) => {
    response.writeHead(200, { "content-type": "application/x-ndjson" });
    response.write(`${JSON.stringify({ type: "text_delta", delta: "你" })}\n`);
    response.write(`${JSON.stringify({ type: "text_delta", delta: "好" })}\n`);
    response.end(`${JSON.stringify({ type: "complete", result: { type: "final", content: "你好" } })}\n`);
  }, async (port) => {
    const events = [];
    const result = await new ModelClient("127.0.0.1", port).generateStream({ messages: [] }, { onEvent: (event) => events.push(event) });
    assert.deepEqual(events.map((event) => event.type), ["text_delta", "text_delta"]);
    assert.equal(events.filter((event) => event.type === "text_delta").map((event) => event.delta).join(""), "你好");
    assert.equal(result.content, "你好");
  });
});

test("non-streaming model JSON preserves Chinese text across response chunks", async () => {
  await withServer((_request, response) => {
    const body = Buffer.from(JSON.stringify({ type: "final", content: "需要结合具体症状核对" }), "utf8");
    const marker = Buffer.from("核", "utf8");
    const markerStart = body.indexOf(marker);
    assert.notEqual(markerStart, -1);
    const split = markerStart + 1;
    response.writeHead(200, { "content-type": "application/json; charset=utf-8" });
    response.write(body.subarray(0, split));
    setTimeout(() => response.end(body.subarray(split)), 5);
  }, async (port) => {
    const result = await new ModelClient("127.0.0.1", port).generate({ messages: [] });
    assert.equal(result.content, "需要结合具体症状核对");
    assert.equal(JSON.stringify(result).includes("�"), false);
  });
});

test("aborting a streaming model request stops later events", async () => {
  await withServer((_request, response) => {
    response.writeHead(200, { "content-type": "application/x-ndjson" });
    response.write(`${JSON.stringify({ type: "text_delta", delta: "partial" })}\n`);
    const timer = setTimeout(() => response.end(`${JSON.stringify({ type: "complete", result: { type: "final", content: "too late" } })}\n`), 250);
    response.on("close", () => clearTimeout(timer));
  }, async (port) => {
    const controller = new AbortController();
    const events = [];
    const pending = new ModelClient("127.0.0.1", port).generateStream({ messages: [] }, {
      signal: controller.signal,
      onEvent: (event) => { events.push(event); controller.abort(); },
    });
    await assert.rejects(pending, (error) => error.code === "cancelled");
    assert.deepEqual(events.map((event) => event.type), ["text_delta"]);
  });
});
