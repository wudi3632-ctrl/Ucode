import { ModelClient } from "../../agent/model/client.js";

export class MeasuredProvider {
  constructor(client, { model = "unknown", mode = "live" } = {}) {
    this.client = client;
    this.model = model;
    this.mode = mode;
    this.calls = [];
  }

  async health() {
    return typeof this.client.health === "function" ? this.client.health() : { status: "ok", runtime: this.mode, model: this.model };
  }

  async generate(payload, options = {}) {
    const started = performance.now();
    const result = await this.client.generate(payload, options);
    this.calls.push({ mode: payload.mode, elapsedMs: performance.now() - started, firstCharacterMs: null });
    return result;
  }

  async generateStream(payload, { signal, onEvent } = {}) {
    const started = performance.now();
    let firstCharacterMs = null;
    const result = typeof this.client.generateStream === "function"
      ? await this.client.generateStream(payload, {
        signal,
        onEvent: (event) => {
          if (firstCharacterMs == null && event?.type === "text_delta" && event.delta) firstCharacterMs = performance.now() - started;
          onEvent?.(event);
        },
      })
      : await this.client.generate(payload, { signal });
    this.calls.push({ mode: payload.mode, elapsedMs: performance.now() - started, firstCharacterMs });
    return result;
  }

  metricsSince(index) {
    const calls = this.calls.slice(index);
    return {
      modelCalls: calls.length,
      firstCharacterMs: calls.find((item) => item.firstCharacterMs != null)?.firstCharacterMs ?? null,
      modelElapsedMs: calls.reduce((sum, item) => sum + item.elapsedMs, 0),
    };
  }
}

export function createLiveProvider(url) {
  const parsed = new URL(url);
  return new MeasuredProvider(new ModelClient(parsed.hostname, Number(parsed.port || 80)), { model: "qwen2.5-coder-1.5b", mode: "live" });
}

export function createScriptedProvider(caseMap) {
  const state = new Map();
  const client = {
    async health() { return { status: "ok", runtime: "scripted-ci", model: "scripted-same-model" }; },
    async generate(payload) {
      const caseId = String(payload.task_id || "").replace(/^benchmark:/, "").split(":")[0];
      const item = caseMap.get(caseId);
      if (!item) return { type: "final", content: "无法识别测试任务。" };
      const key = `${caseId}:${payload.mode}`;
      const count = state.get(key) || 0;
      state.set(key, count + 1);
      if (payload.requested_tool?.name === "write_file") {
        const target = payload.requested_tool.arguments.path;
        return { type: "tool_request", content: "", tool: { name: "write_file", arguments: { path: target, content: item.solution?.[target] || item.files[target] || "# generated\n" } } };
      }
      if (item.kind === "action" && payload.tools?.length) {
        const changes = Object.entries(item.solution || {}).map(([path, content]) => ({ path, operation: Object.hasOwn(item.files, path) ? "modify" : "create", content }));
        return changes.length === 1
          ? { type: "tool_request", content: "", tool: { name: "write_file", arguments: { path: changes[0].path, content: changes[0].content } } }
          : { type: "tool_request", content: "", tool: { name: "write_files", arguments: { changes } } };
      }
      if (item.kind === "action") return { type: "final", content: JSON.stringify({ changes: Object.entries(item.solution || {}).map(([path, content]) => ({ path, content })) }) };
      if (caseId === "exact-reply") return { type: "final", content: "harness_ok" };
      if (caseId === "greeting") return { type: "final", content: "你好，我可以帮助理解代码、分析项目并安全地完成修改。" };
      if (caseId === "missing-file") return { type: "final", content: "项目索引中不存在 missing.py。" };
      if (caseId === "path-traversal") return { type: "final", content: "不能读取项目边界之外的路径。" };
      if (caseId === "duplicate-basename") return { type: "final", content: "存在 src/config.py 和 tests/config.py，请指定相对路径。" };
      if (caseId === "project-overview") return { type: "final", content: "入口是 src/app.py，数据存储在 src/store.py，测试位于 tests/test_store.py。" };
      if (caseId === "explain-file") return { type: "final", content: "total 将 price 乘以 quantity，再按 discount 比例扣减。" };
      return { type: "final", content: "已根据给定证据回答。" };
    },
  };
  client.generateStream = async (payload, { onEvent } = {}) => {
    const result = await client.generate(payload);
    if (result.type === "final" && result.content) onEvent?.({ type: "text_delta", delta: result.content });
    return result;
  };
  return new MeasuredProvider(client, { model: "scripted-same-model", mode: "scripted" });
}

export function withCallBudget(provider, maximum = 6) {
  let calls = 0;
  const claim = () => {
    calls += 1;
    if (calls > maximum) throw new Error(`model call budget exceeded (${maximum})`);
  };
  return {
    get callsUsed() { return calls; },
    async generate(payload, options) { claim(); return provider.generate(payload, options); },
    async generateStream(payload, options) { claim(); return provider.generateStream(payload, options); },
  };
}
