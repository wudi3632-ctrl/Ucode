import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createHash, randomUUID } from "node:crypto";
import { Store } from "../store/store.js";
import { AuditLog } from "../audit/audit.js";
import { ProjectTools } from "../tools/tools.js";
import { ModelClient } from "../model/client.js";
import { OpenAICompatibleClient } from "../model/openai_client.js";
import { AgentService } from "../executor/service.js";
import { EXECUTOR_PROTOCOL_V1, validateExecutorEvent, validateExecutorRequest } from "../executor/protocol.js";
import { StatelessExecutor } from "../executor/stateless.js";
import { AppError, errorBody } from "../shared/errors.js";
import { log } from "../shared/log.js";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const defaults = JSON.parse(fs.readFileSync(path.join(projectRoot, "config/default.json"), "utf8"));
const config = {
  ...defaults,
  host: process.env.POCKETMIND_HOST || defaults.host,
  port: Number(process.env.POCKETMIND_PORT || defaults.port),
  modelHost: process.env.POCKETMIND_MODEL_HOST || defaults.modelHost,
  modelPort: Number(process.env.POCKETMIND_MODEL_PORT || defaults.modelPort),
  coderPort: process.env.POCKETMIND_CODER_PORT ? Number(process.env.POCKETMIND_CODER_PORT) : null,
};
if (config.host !== "127.0.0.1" && config.host !== "localhost") throw new Error("PocketMind only binds to loopback");
const installRoot = process.env.POCKETMIND_ROOT ? path.resolve(process.env.POCKETMIND_ROOT) : projectRoot;
const dataDir = path.resolve(installRoot, config.dataDir);
const service = new AgentService({
  store: new Store(dataDir),
  audit: new AuditLog(dataDir),
  tools: new ProjectTools({ timeoutMs: config.commandTimeoutMs, maxOutputBytes: config.maxOutputBytes, pythonExecutable: process.env.POCKETMIND_PYTHON || "python" }),
  model: new ModelClient(config.modelHost, config.modelPort),
  coder: config.coderPort ? new ModelClient(config.modelHost, config.coderPort) : null,
  dataDir,
});
const statelessExecutor = new StatelessExecutor({ tools: service.tools, coder: () => service.activeCoder(), controller: service.model });
const staticDir = path.join(projectRoot, "agent/static");
const activeRequests = new Map();
const requestRuns = new Map();
const executorRuns = new Map();
let providerState = Object.freeze({ mode: "bundled", runtime: "bundled-local", model: "bundled-coder", baseUrl: null });

async function configureProvider(input) {
  if (!input || typeof input !== "object" || Array.isArray(input)) throw new AppError("provider_invalid", "Provider configuration must be an object", 400);
  const mode = String(input.mode || "").trim();
  if (mode === "bundled") {
    service.setExternalCoder(null);
    providerState = Object.freeze({ mode: "bundled", runtime: "bundled-local", model: "bundled-coder", baseUrl: null });
    return providerState;
  }
  if (mode !== "openai-compatible") throw new AppError("provider_invalid", "Unsupported provider mode", 400);
  const baseUrl = String(input.baseUrl || "").trim();
  const model = String(input.model || "").trim();
  const apiKey = String(input.apiKey || "");
  if (baseUrl.length > 2_048 || model.length > 200 || apiKey.length > 8_192) throw new AppError("provider_invalid", "Provider configuration is too long", 400);
  const client = new OpenAICompatibleClient({ baseUrl, model, apiKey, allowNetwork: input.allowNetwork === true });
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 10_000);
  timer.unref?.();
  let health;
  try { health = await client.health({ signal: controller.signal }); }
  finally { clearTimeout(timer); }
  service.setExternalCoder(client);
  providerState = Object.freeze({ mode, runtime: health.runtime, model: health.model, baseUrl: health.baseUrl });
  return providerState;
}

function emitRun(run, event) {
  const value = { requestId: run.requestId, ticketId: run.ticketId, sequence: ++run.sequence, timestamp: new Date().toISOString(), ...event };
  run.events.push(value);
  for (const response of run.subscribers) {
    if (!response.writableEnded && !response.destroyed) response.write(`${JSON.stringify(value)}\n`);
  }
  return value;
}

function subscribeRun(run, response, afterSequence) {
  response.writeHead(200, { "content-type": "application/x-ndjson; charset=utf-8", "cache-control": "no-store", "x-request-id": run.requestId });
  for (const event of run.events) {
    if (event.sequence > afterSequence) response.write(`${JSON.stringify(event)}\n`);
  }
  if (run.terminal) {
    response.end();
    return;
  }
  run.subscribers.add(response);
  response.on("close", () => run.subscribers.delete(response));
}

function finishRun(run, event) {
  if (run.terminal) return;
  run.terminal = emitRun(run, event);
  for (const response of run.subscribers) if (!response.writableEnded) response.end();
  run.subscribers.clear();
  if (activeRequests.get(run.ticketId) === run) activeRequests.delete(run.ticketId);
  const cleanup = setTimeout(() => requestRuns.delete(run.requestId), 5 * 60_000);
  cleanup.unref?.();
}

function createRun(ticketId) {
  const run = { ticketId, requestId: randomUUID(), controller: new AbortController(), sequence: 0, events: [], subscribers: new Set(), terminal: null };
  activeRequests.get(ticketId)?.controller.abort();
  activeRequests.set(ticketId, run);
  requestRuns.set(run.requestId, run);
  return run;
}

function executeRun(run) {
  emitRun(run, { type: "start" });
  void service.plan(run.ticketId, { signal: run.controller.signal, onEvent: (event) => emitRun(run, event) })
    .then((result) => finishRun(run, { type: run.controller.signal.aborted ? "cancelled" : "complete", result }))
    .catch((error) => finishRun(run, { type: run.controller.signal.aborted || error.code === "cancelled" ? "cancelled" : "error", error: { code: error.code || "internal_error", message: error.message } }));
}

function emitExecutorRun(run, payload) {
  const event = validateExecutorEvent({
    version: EXECUTOR_PROTOCOL_V1,
    requestId: run.requestId,
    turnId: run.turnId,
    sequence: ++run.sequence,
    timestamp: new Date().toISOString(),
    ...payload,
  }, run.request);
  run.events.push(event);
  for (const response of run.subscribers) {
    if (!response.writableEnded && !response.destroyed) response.write(`${JSON.stringify(event)}\n`);
  }
  return event;
}

function finishExecutorRun(run, payload) {
  if (run.terminal) return;
  run.terminal = emitExecutorRun(run, payload);
  for (const response of run.subscribers) if (!response.writableEnded) response.end();
  run.subscribers.clear();
  const cleanup = setTimeout(() => executorRuns.delete(run.requestId), 5 * 60_000);
  cleanup.unref?.();
}

function createExecutorRun(request) {
  const run = {
    request,
    requestId: request.requestId,
    turnId: request.turnId,
    controller: new AbortController(),
    sequence: 0,
    events: [],
    subscribers: new Set(),
    terminal: null,
  };
  executorRuns.set(run.requestId, run);
  return run;
}

function executeExecutorRun(run) {
  emitExecutorRun(run, { type: "start" });
  void statelessExecutor.execute(run.request, {
    signal: run.controller.signal,
    onEvent: (event) => emitExecutorRun(run, event),
  }).then((content) => {
    if (run.controller.signal.aborted) return finishExecutorRun(run, { type: "cancelled" });
    finishExecutorRun(run, {
      type: "complete",
      content,
      contentDigest: createHash("sha256").update(content).digest("hex"),
    });
  }).catch((error) => {
    if (run.controller.signal.aborted || error.code === "cancelled") return finishExecutorRun(run, { type: "cancelled" });
    finishExecutorRun(run, {
      type: "failure",
      code: error.code || "executor_internal",
      message: error.message || "Executor failed",
      retryable: !["executor_action_unavailable", "executor_authority_mismatch", "executor_protocol"].includes(error.code),
    });
  });
}

function json(response, status, body, requestId) {
  response.writeHead(status, { "content-type": "application/json; charset=utf-8", "cache-control": "no-store", "x-request-id": requestId, "x-content-type-options": "nosniff" });
  response.end(JSON.stringify(body));
}

async function body(request) {
  let value = "";
  for await (const chunk of request) {
    value += chunk;
    if (Buffer.byteLength(value) > 1024 * 1024) throw new AppError("body_too_large", "Request body exceeds 1 MiB", 413);
  }
  try { return value ? JSON.parse(value) : {}; }
  catch { throw new AppError("invalid_json", "Request body must be valid JSON"); }
}

function assertLocalOrigin(request) {
  const origin = request.headers.origin;
  if (!origin) return;
  const allowed = new Set([`http://127.0.0.1:${config.port}`, `http://localhost:${config.port}`]);
  if (!allowed.has(origin)) throw new AppError("origin_denied", "Cross-origin requests are denied", 403);
}

const server = http.createServer(async (request, response) => {
  const requestId = randomUUID();
  try {
    assertLocalOrigin(request);
    const url = new URL(request.url, `http://${request.headers.host || "localhost"}`);
    const segments = url.pathname.split("/").filter(Boolean);
    let result;
    if (request.method === "GET" && url.pathname === "/health") {
      let model = { status: "unavailable" };
      try { model = await service.model.health(); } catch {}
      return json(response, 200, { status: "ok", model, provider: providerState }, requestId);
    }
    if (request.method === "GET" && url.pathname === "/ready") return json(response, 200, { status: "ready" }, requestId);
    if (request.method === "POST" && segments[0] === "v3" && segments[1] === "executor" && segments[2] === "runs" && segments[4] === "cancel") {
      const run = executorRuns.get(segments[3]);
      if (run && !run.terminal) run.controller.abort();
      return json(response, 200, { status: run && !run.terminal ? "cancelling" : "idle" }, requestId);
    }
    if (request.method === "POST" && segments.length === 3 && segments[0] === "v3" && segments[1] === "executor" && segments[2] === "runs") {
      const input = validateExecutorRequest(await body(request));
      const afterSequence = Math.max(0, Number.parseInt(url.searchParams.get("afterSequence") || "0", 10) || 0);
      let run = executorRuns.get(input.requestId);
      if (run && run.turnId !== input.turnId) throw new AppError("executor_request_conflict", "requestId already belongs to another Turn", 409);
      const isNew = !run;
      if (!run) run = createExecutorRun(input);
      subscribeRun(run, response, afterSequence);
      if (isNew) executeExecutorRun(run);
      return;
    }
    if (request.method === "POST" && segments[0] === "v2" && segments[1] === "tickets" && segments[3] === "cancel") {
      const active = activeRequests.get(segments[2]);
      if (active) active.controller.abort();
      return json(response, 200, { status: active ? "cancelling" : "idle" }, requestId);
    }
    if (request.method === "POST" && segments[0] === "v2" && segments[1] === "tickets" && segments[3] === "stream") {
      const input = await body(request);
      const afterSequence = Number.isSafeInteger(input.afterSequence) && input.afterSequence >= 0 ? input.afterSequence : 0;
      let run = typeof input.requestId === "string" ? requestRuns.get(input.requestId) : activeRequests.get(segments[2]);
      if (input.requestId && (!run || run.ticketId !== segments[2])) throw new AppError("stream_expired", "Stream replay window has expired", 410);
      const isNew = !run;
      if (!run) run = createRun(segments[2]);
      subscribeRun(run, response, afterSequence);
      if (isNew) executeRun(run);
      return;
    }
    if (request.method === "POST" && url.pathname === "/api/provider/configure") result = await configureProvider(await body(request));
    else if (request.method === "POST" && url.pathname === "/api/projects/select") result = service.selectProject((await body(request)).path);
    else if (request.method === "GET" && url.pathname === "/api/state") result = { projectRoot: service.store.state.projectRoot, tickets: service.listTickets() };
    else if (request.method === "POST" && url.pathname === "/api/tickets") result = service.createTicket((await body(request)).request);
    else if (request.method === "GET" && segments[0] === "api" && segments[1] === "tickets" && segments.length === 3) result = service.ticket(segments[2]);
    else if (request.method === "POST" && segments[0] === "api" && segments[1] === "tickets" && segments[3] === "messages") result = service.continueTicket(segments[2], (await body(request)).content);
    else if (request.method === "POST" && segments[0] === "api" && segments[1] === "tickets" && segments[3] === "context") result = service.hydrateTicket(segments[2], (await body(request)).messages);
    else if (request.method === "POST" && segments[0] === "api" && segments[1] === "tickets" && segments[3] === "skills") result = service.setSkillContext(segments[2], (await body(request)).context);
    else if (request.method === "POST" && segments[0] === "api" && segments[1] === "tickets" && segments[3] === "attachments") result = service.setAttachmentContext(segments[2], (await body(request)).attachments);
    else if (request.method === "POST" && segments[0] === "api" && segments[1] === "tickets" && segments[3] === "plan") result = await service.plan(segments[2]);
    else if (request.method === "POST" && segments[0] === "api" && segments[1] === "tickets" && segments[3] === "tools") {
      const input = await body(request); result = await service.callTool(segments[2], input.name, input.arguments);
    } else if (request.method === "POST" && segments[0] === "api" && segments[1] === "tickets" && segments[3] === "effects" && segments[5] === "approval") {
      const input = await body(request);
      result = service.approve(segments[2], segments[4], Boolean(input.approved), Array.isArray(input.selectedPaths) ? input.selectedPaths : null);
    } else if (request.method === "POST" && segments[0] === "api" && segments[1] === "tickets" && segments[3] === "effects" && segments[5] === "rollback") {
      result = service.rollback(segments[2], segments[4]);
    } else if (request.method === "POST" && segments[0] === "api" && segments[1] === "tickets" && segments[3] === "audit" && segments[4] === "export") result = service.exportAudit(segments[2]);
    else if (request.method === "GET" && segments[0] === "api" && segments[1] === "tickets" && segments[3] === "escalation") result = service.escalation(segments[2]);
    else if (request.method === "GET" && (url.pathname === "/" || url.pathname === "/index.html")) {
      response.writeHead(200, { "content-type": "text/html; charset=utf-8", "content-security-policy": "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'", "x-content-type-options": "nosniff" });
      return response.end(fs.readFileSync(path.join(staticDir, "index.html")));
    } else if (request.method === "GET" && ["/app.js", "/styles.css"].includes(url.pathname)) {
      const file = path.join(staticDir, url.pathname.slice(1));
      response.writeHead(200, { "content-type": url.pathname.endsWith(".js") ? "text/javascript; charset=utf-8" : "text/css; charset=utf-8" });
      return response.end(fs.readFileSync(file));
    } else throw new AppError("not_found", "Route not found", 404);
    json(response, 200, result, requestId);
  } catch (error) {
    log("error", "request.failed", { requestId, method: request.method, url: request.url, code: error.code, message: error.message });
    json(response, error.status || 500, errorBody(error, requestId), requestId);
  }
});

server.listen(config.port, config.host, () => log("info", "server.ready", { host: config.host, port: config.port }));
const shutdown = () => server.close(() => process.exit(0));
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
