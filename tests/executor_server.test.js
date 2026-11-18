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

async function postJsonSplitInsideUtf8(port, value, needle) {
  const body = Buffer.from(JSON.stringify(value), "utf8");
  const marker = Buffer.from(needle, "utf8");
  const markerStart = body.indexOf(marker);
  assert.notEqual(markerStart, -1);
  const split = markerStart + 1;
  return new Promise((resolve, reject) => {
    const request = http.request({
      host: "127.0.0.1",
      port,
      path: "/v3/executor/runs",
      method: "POST",
      headers: { "content-type": "application/json", "content-length": body.length },
    }, (response) => {
      response.setEncoding("utf8");
      let output = "";
      response.on("data", (chunk) => { output += chunk; });
      response.on("end", () => resolve({ status: response.statusCode, output }));
    });
    request.on("error", reject);
    request.write(body.subarray(0, split));
    setTimeout(() => request.end(body.subarray(split)), 5);
  });
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


  const rescueTitle = "存在未运行的自动服务，需要结合具体症状核对";
  const rescueRequest = {
    version: "ucode.executor.v1",
    requestId: `rescue-${Date.now()}`,
    turnId: "turn-rescue-utf8",
    threadKey: "desktop:conversation-rescue",
    mode: "rescue",
    assignment: null,
    mandate: null,
    inputs: [{ id: "input-rescue", kind: "trigger", triggerId: "trigger-rescue", text: "只读诊断", human: true }],
    history: [],
    workspace: { canonicalRoot: project, fingerprint: "fixture", files: [] },
    attachmentToken: null,
    attachments: [],
    metadata: {
      taskContext: {
        rescueEvidence: {
          version: "ucode.rescue.diagnosis.v1",
          outcome: "diagnosed",
          mutationPerformed: false,
          evidence: [{ id: "evidence-1", toolId: "system.service.inspect", state: "observed", summary: "自动服务状态已读取" }],
          hypotheses: [{ id: "service.auto", state: "supported", title: rescueTitle, evidenceIds: ["evidence-1"] }],
        },
      },
    },
  };
  const rescueResponse = await postJsonSplitInsideUtf8(agentPort, rescueRequest, "核");
  assert.equal(rescueResponse.status, 200);
  const rescueEvents = rescueResponse.output.trim().split("\n").map((line) => JSON.parse(line));
  assert.equal(rescueEvents.map((event) => event.type).join(","), "start,text_delta,complete");
  assert.match(rescueEvents[1].delta, new RegExp(rescueTitle));
  assert.match(rescueEvents[2].content, new RegExp(rescueTitle));
  assert.equal(rescueResponse.output.includes("�"), false);
});
