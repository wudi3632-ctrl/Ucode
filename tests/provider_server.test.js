import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import test from "node:test";

async function freePort() {
  const server = http.createServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return port;
}

async function waitReady(url, child, logs) {
  const deadline = Date.now() + 8_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`agent exited early: ${logs.join("")}`);
    try { if ((await fetch(url)).ok) return; } catch {}
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`agent did not become ready: ${logs.join("")}`);
}

async function post(url, body) {
  return fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
}

test("provider configure endpoint switches safely and never exposes API keys", async () => {
  let authorization = "";
  const provider = http.createServer(async (request, response) => {
    authorization = String(request.headers.authorization || "");
    if (request.url === "/v1/chat/completions") {
      let body = "";
      for await (const chunk of request) body += chunk;
      assert.equal(JSON.parse(body).model, "fake-model");
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.write('data: {"choices":[{"delta":{"content":"外部"}}]}\n\n');
      response.write('data: {"choices":[{"delta":{"content":"回答"}}]}\n\n');
      return response.end("data: [DONE]\n\n");
    }
    response.writeHead(200, { "content-type": "application/json" });
    response.end('{"data":[]}');
  });
  await new Promise((resolve) => provider.listen(0, "127.0.0.1", resolve));
  const providerPort = provider.address().port;
  const agentPort = await freePort();
  const tempRoot = await mkdtemp(path.join(os.tmpdir(), "ucode-provider-"));
  const workspace = path.join(tempRoot, "workspace");
  await mkdir(workspace);
  await writeFile(path.join(workspace, "README.md"), "# provider test\n", "utf8");
  const logs = [];
  const child = spawn(process.execPath, ["agent/runtime/server.js"], {
    cwd: path.resolve(import.meta.dirname, ".."),
    env: { ...process.env, POCKETMIND_PORT: String(agentPort), POCKETMIND_ROOT: tempRoot },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stdout.on("data", (chunk) => logs.push(chunk.toString()));
  child.stderr.on("data", (chunk) => logs.push(chunk.toString()));
  try {
    const base = `http://127.0.0.1:${agentPort}`;
    await waitReady(`${base}/ready`, child, logs);
    const secret = "integration-secret-must-not-leak";
    const configured = await post(`${base}/api/provider/configure`, { mode: "openai-compatible", baseUrl: `http://127.0.0.1:${providerPort}/v1`, model: "fake-model", apiKey: secret, allowNetwork: false });
    assert.equal(configured.status, 200);
    const configuredBody = await configured.json();
    assert.equal(configuredBody.model, "fake-model");
    assert.equal(JSON.stringify(configuredBody).includes(secret), false);
    assert.equal(authorization, `Bearer ${secret}`);

    const healthBody = await (await fetch(`${base}/health`)).json();
    assert.equal(healthBody.provider.mode, "openai-compatible");
    assert.equal(healthBody.provider.model, "fake-model");
    assert.equal(JSON.stringify(healthBody).includes(secret), false);

    assert.equal((await post(`${base}/api/projects/select`, { path: workspace })).status, 200);
    const ticket = await (await post(`${base}/api/tickets`, { request: "用两句话解释闭包和词法作用域的区别" })).json();
    const streamed = await (await post(`${base}/v2/tickets/${ticket.id}/stream`, {})).text();
    const events = streamed.trim().split(/\r?\n/).map((line) => JSON.parse(line));
    assert.equal(events.filter((event) => event.type === "text_delta").map((event) => event.delta).join(""), "外部回答");
    assert.equal(events.at(-1).result.content, "外部回答");
    assert.equal(authorization, `Bearer ${secret}`);

    const denied = await post(`${base}/api/provider/configure`, { mode: "openai-compatible", baseUrl: "https://example.com/v1", model: "remote", apiKey: secret, allowNetwork: false });
    assert.equal(denied.status, 403);
    const afterDenied = await (await fetch(`${base}/health`)).json();
    assert.equal(afterDenied.provider.model, "fake-model");

    const bundled = await post(`${base}/api/provider/configure`, { mode: "bundled" });
    assert.equal(bundled.status, 200);
    const bundledBody = await bundled.json();
    assert.equal(bundledBody.mode, "bundled");
    assert.equal(JSON.stringify(bundledBody).includes(secret), false);
  } finally {
    child.kill();
    await new Promise((resolve) => child.once("exit", resolve));
    await new Promise((resolve) => provider.close(resolve));
    await rm(tempRoot, { recursive: true, force: true });
  }
});
