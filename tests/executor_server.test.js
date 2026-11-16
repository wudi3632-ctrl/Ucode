import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";

async function listen(server) {
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return server.address().port;
}

async function freePort() {
  const server = http.createServer();
  const port = await listen(server);
  await new Promise((resolve) => server.close(resolve));
  return port;
}

async function waitReady(port, child, stderr) {
  for (let attempt = 0; attempt < 80; attempt += 1) {
    if (child.exitCode != null) throw new Error(`Agent exited early: ${stderr.value}`);
    try {
      const response = await fetch(`http://127.0.0.1:${port}/ready`);
      if (response.ok) return;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error(`Agent did not become ready: ${stderr.value}`);
}

test("v3 Executor endpoint streams typed chat events without creating a Node ticket", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ucode-executor-server-"));
  const project = path.join(root, "project");
  fs.mkdirSync(project, { recursive: true });
  fs.writeFileSync(path.join(project, "README.md"), "# fixture");
  const model = http.createServer(async (request, response) => {
    if (request.url === "/generate/stream") {
      for await (const _chunk of request) {}
      response.writeHead(200, { "content-type": "application/x-ndjson" });
      response.write(`${JSON.stringify({ type: "text_delta", delta: "你好" })}\n`);
      response.end(`${JSON.stringify({ type: "complete", result: { type: "final", content: "你好，这是无状态回答。", tool: null } })}\n`);
      return;
    }
    response.writeHead(200, { "content-type": "application/json" });
    response.end('{"status":"ready"}');
  });
  const modelPort = await listen(model);
  const agentPort = await freePort();
  const stderr = { value: "" };
  const child = spawn(process.execPath, ["agent/runtime/server.js"], {
    cwd: path.resolve("."),
    windowsHide: true,
    env: {
      ...process.env,
      POCKETMIND_ROOT: root,
      POCKETMIND_PORT: String(agentPort),
      POCKETMIND_MODEL_PORT: String(modelPort),
      POCKETMIND_CODER_PORT: String(modelPort),
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr.on("data", (chunk) => { stderr.value += chunk.toString(); });
  t.after(async () => {
    child.kill();
    await new Promise((resolve) => child.once("exit", resolve));
    await new Promise((resolve) => model.close(resolve));
    fs.rmSync(root, { recursive: true, force: true });
  });
  await waitReady(agentPort, child, stderr);

  const marker = `request-${Date.now()}`;
  const response = await fetch(`http://127.0.0.1:${agentPort}/v3/executor/runs`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      version: "ucode.executor.v1",
      requestId: marker,
      turnId: "turn-live",
      threadKey: "desktop:conversation-live",
      mode: "chat",
      assignment: null,
      mandate: null,
      inputs: [{ id: "input-live", kind: "trigger", triggerId: "trigger-live", text: "你好", human: true }],
      history: [],
      workspace: { canonicalRoot: project, fingerprint: "fixture", files: [{ path: "README.md", sizeBytes: 9, contentDigest: null }] },
      attachmentToken: null,
      attachments: [],
      metadata: {},
    }),
  });
  assert.equal(response.status, 200);
  const events = (await response.text()).trim().split("\n").map((line) => JSON.parse(line));
  assert.deepEqual(events.map((event) => event.type), ["start", "text_delta", "complete"]);
  assert.deepEqual(events.map((event) => event.sequence), [1, 2, 3]);
  assert.equal(events[2].content, "你好，这是无状态回答。");
  assert.match(events[2].contentDigest, /^[0-9a-f]{64}$/);
  const statePath = path.join(root, "data", "state.json");
  if (fs.existsSync(statePath)) {
    const state = JSON.parse(fs.readFileSync(statePath, "utf8"));
    assert.deepEqual(state.tickets || [], []);
  }
});
