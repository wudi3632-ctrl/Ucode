import assert from "node:assert/strict";
import http from "node:http";
import test from "node:test";
import { OpenAICompatibleClient } from "../agent/model/openai_client.js";

async function withServer(handler, run) {
  const server = http.createServer(handler);
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try { await run(server.address().port); }
  finally { await new Promise((resolve) => server.close(resolve)); }
}

async function readJson(request) {
  let body = "";
  for await (const chunk of request) body += chunk;
  return JSON.parse(body || "{}");
}

test("OpenAI-compatible JSON completion includes bounded workspace context", async () => {
  let captured;
  await withServer(async (request, response) => {
    if (request.url === "/v1/models") {
      response.writeHead(200, { "content-type": "application/json" });
      return response.end('{"data":[]}');
    }
    captured = { authorization: request.headers.authorization, body: await readJson(request) };
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ choices: [{ message: { role: "assistant", content: "真实回答" } }] }));
  }, async (port) => {
    const client = new OpenAICompatibleClient({ baseUrl: `http://127.0.0.1:${port}/v1`, model: "local-model", apiKey: "secret-value" });
    const health = await client.health();
    assert.deepEqual(health, { status: "ready", runtime: "openai-compatible", model: "local-model", baseUrl: `http://127.0.0.1:${port}/v1/` });
    assert.equal(JSON.stringify(health).includes("secret-value"), false);
    const result = await client.generate({ messages: [{ role: "user", content: "解释" }], workspace: { root: "C:/project", files: [{ path: "src/a.js", content: "export const a = 1;" }] }, tools: [] });
    assert.equal(result.content, "真实回答");
    assert.equal(captured.authorization, "Bearer secret-value");
    assert.equal(captured.body.model, "local-model");
    assert.match(captured.body.messages[0].content, /src\/a\.js/);
    assert.match(captured.body.messages[0].content, /export const a = 1/);
  });
});

test("OpenAI-compatible SSE preserves text delta order", async () => {
  await withServer(async (request, response) => {
    await readJson(request);
    response.writeHead(200, { "content-type": "text/event-stream" });
    response.write('data: {"choices":[{"delta":{"content":"你"}}]}\r\n\r\n');
    response.write('data: {"choices":[{"delta":{"content":"好"}}]}\n\n');
    response.end("data: [DONE]\n\n");
  }, async (port) => {
    const events = [];
    const client = new OpenAICompatibleClient({ baseUrl: `http://localhost:${port}/v1/`, model: "stream-model" });
    const result = await client.generateStream({ messages: [{ role: "user", content: "hi" }] }, { onEvent: (event) => events.push(event) });
    assert.equal(events.map((event) => event.delta).join(""), "你好");
    assert.equal(result.content, "你好");
  });
});

test("OpenAI-compatible SSE reconstructs fragmented tool calls", async () => {
  await withServer(async (request, response) => {
    const body = await readJson(request);
    assert.equal(body.tool_choice.function.name, "write_file");
    response.writeHead(200, { "content-type": "text/event-stream" });
    response.write('data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"write_","arguments":"{\\\"path\\\":\\\"a.js\\\","}}]}}]}\n');
    response.write('data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"file","arguments":"\\\"content\\\":\\\"ok\\\"}"}}]}}]}\n');
    response.end("data: [DONE]\n");
  }, async (port) => {
    const client = new OpenAICompatibleClient({ baseUrl: `http://127.0.0.1:${port}/v1/`, model: "tool-model" });
    const result = await client.generateStream({ messages: [], tools: [{ type: "function", function: { name: "write_file", parameters: { type: "object" } } }], requested_tool: { name: "write_file" } });
    assert.deepEqual(result, { type: "tool_request", content: "", tool: { name: "write_file", arguments: { path: "a.js", content: "ok" } } });
  });
});

test("OpenAI-compatible streaming honors AbortSignal", async () => {
  await withServer(async (request, response) => {
    await readJson(request);
    response.writeHead(200, { "content-type": "text/event-stream" });
    response.write('data: {"choices":[{"delta":{"content":"partial"}}]}\n');
    const timer = setTimeout(() => response.end("data: [DONE]\n"), 1_000);
    response.on("close", () => clearTimeout(timer));
  }, async (port) => {
    const controller = new AbortController();
    const client = new OpenAICompatibleClient({ baseUrl: `http://127.0.0.1:${port}/v1/`, model: "cancel-model" });
    const pending = client.generateStream({ messages: [] }, { signal: controller.signal, onEvent: () => controller.abort() });
    await assert.rejects(pending, (error) => error.code === "cancelled");
  });
});

test("OpenAI-compatible endpoint policy rejects unsafe and network URLs by default", () => {
  for (const baseUrl of ["file:///tmp/v1", "http://user:pass@localhost/v1", "http://localhost/v1?token=x", "http://localhost/v1#x"]) {
    assert.throws(() => new OpenAICompatibleClient({ baseUrl, model: "x" }), (error) => error.code === "provider_invalid");
  }
  assert.throws(() => new OpenAICompatibleClient({ baseUrl: "https://example.com/v1", model: "x" }), (error) => error.code === "provider_network_denied");
  assert.doesNotThrow(() => new OpenAICompatibleClient({ baseUrl: "https://example.com/v1", model: "x", allowNetwork: true }));
});
