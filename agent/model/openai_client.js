import { AppError } from "../shared/errors.js";

const MAX_WORKSPACE_BYTES = 120_000;

export class OpenAICompatibleClient {
  constructor({ baseUrl, model, apiKey = "", allowNetwork = false }) {
    this.baseUrl = normalizeBaseUrl(baseUrl, allowNetwork);
    this.model = String(model || "").trim();
    this.apiKey = String(apiKey || "").trim();
    if (!this.model) throw new AppError("provider_invalid", "Model name is required", 400);
  }

  headers(extra = {}) {
    return { "content-type": "application/json", ...(this.apiKey ? { authorization: `Bearer ${this.apiKey}` } : {}), ...extra };
  }

  async health({ signal } = {}) {
    try {
      const response = await fetch(new URL("models", this.baseUrl), { headers: this.headers(), signal });
      if (!response.ok) throw await providerError(response);
      return { status: "ready", runtime: "openai-compatible", model: this.model, baseUrl: redactedUrl(this.baseUrl) };
    } catch (error) { throw normalizeRequestError(error, signal); }
  }

  async generate(payload, { signal } = {}) {
    try {
      const response = await fetch(new URL("chat/completions", this.baseUrl), {
        method: "POST", headers: this.headers(), signal, body: JSON.stringify(requestBody(payload, this.model, false)),
      });
      if (!response.ok) throw await providerError(response);
      return normalizeMessage((await response.json()).choices?.[0]?.message);
    } catch (error) { throw normalizeRequestError(error, signal); }
  }

  async generateStream(payload, { signal, onEvent } = {}) {
    let response;
    try {
      response = await fetch(new URL("chat/completions", this.baseUrl), {
        method: "POST", headers: this.headers({ accept: "text/event-stream" }), signal, body: JSON.stringify(requestBody(payload, this.model, true)),
      });
    } catch (error) { throw normalizeRequestError(error, signal); }
    if (!response.ok) throw await providerError(response);
    if (!response.body) throw new AppError("provider_protocol_error", "Provider returned no stream", 503);
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = ""; let content = ""; const toolCalls = [];
    const consume = (line) => {
      if (!line.startsWith("data:")) return false;
      const data = line.slice(5).trim();
      if (data === "[DONE]") return true;
      const chunk = JSON.parse(data);
      const delta = chunk.choices?.[0]?.delta || {};
      if (typeof delta.content === "string" && delta.content) { content += delta.content; onEvent?.({ type: "text_delta", delta: delta.content }); }
      for (const call of delta.tool_calls || []) {
        const index = call.index || 0;
        toolCalls[index] ||= { id: call.id || `call-${index}`, function: { name: "", arguments: "" } };
        if (call.function?.name) toolCalls[index].function.name += call.function.name;
        if (call.function?.arguments) toolCalls[index].function.arguments += call.function.arguments;
      }
      return false;
    };
    try {
      let done = false;
      while (!done) {
        const part = await reader.read();
        if (part.done) break;
        buffer += decoder.decode(part.value, { stream: true });
        let index;
        while ((index = buffer.indexOf("\n")) >= 0) { const line = buffer.slice(0, index).trimEnd(); buffer = buffer.slice(index + 1); done ||= consume(line); }
      }
      if (buffer.trim()) consume(buffer.trim());
      return normalizeMessage({ content, tool_calls: toolCalls.filter(Boolean) });
    } catch (error) {
      throw normalizeRequestError(error, signal);
    } finally { reader.releaseLock(); }
  }
}

function requestBody(payload, model, stream) {
  const messages = normalizeMessages(payload.messages || [], payload.workspace);
  const body = { model, messages, stream, temperature: 0.2 };
  if (Array.isArray(payload.tools) && payload.tools.length) body.tools = payload.tools;
  if (payload.requested_tool?.name) body.tool_choice = { type: "function", function: { name: payload.requested_tool.name } };
  return body;
}

function normalizeMessages(messages, workspace) {
  const output = messages.filter((message) => ["system", "user", "assistant"].includes(message.role)).map((message) => {
    if (!message.tool_calls?.length) return { role: message.role, content: String(message.content || "") };
    return { role: "assistant", content: String(message.content || ""), tool_calls: message.tool_calls.map((call, index) => ({ id: call.id || `call-${index}`, type: "function", function: { name: call.name, arguments: JSON.stringify(call.arguments || {}) } })) };
  });
  const files = Array.isArray(workspace?.files) ? workspace.files : [];
  if (files.length) {
    let context = `Workspace root: ${workspace.root || ""}\n`;
    for (const file of files) {
      const next = typeof file.content === "string" ? `\n--- ${file.path} ---\n${file.content}` : `\n- ${file.path}`;
      if (Buffer.byteLength(context + next, "utf8") > MAX_WORKSPACE_BYTES) break;
      context += next;
    }
    output.unshift({ role: "system", content: `以下是 UCode 已安全索引或读取的真实工作区上下文。不得猜测未提供的文件。\n${context}` });
  }
  return output;
}

function normalizeMessage(message = {}) {
  const call = message.tool_calls?.[0];
  if (call?.function?.name) {
    let args;
    try { args = JSON.parse(call.function.arguments || "{}"); } catch { throw new AppError("provider_protocol_error", "Provider returned invalid tool arguments", 503); }
    return { type: "tool_request", content: String(message.content || ""), tool: { name: call.function.name, arguments: args } };
  }
  return { type: "final", content: String(message.content || ""), tool: null };
}

export function normalizeBaseUrl(value, allowNetwork = false) {
  let url;
  try { url = new URL(String(value || "")); } catch { throw new AppError("provider_invalid", "Provider URL is invalid", 400); }
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password || url.search || url.hash) throw new AppError("provider_invalid", "Provider URL must be a plain HTTP(S) endpoint", 400);
  const hostname = url.hostname.toLowerCase();
  if (!allowNetwork && !["127.0.0.1", "localhost", "[::1]"].includes(hostname)) {
    throw new AppError("provider_network_denied", "Network providers are disabled; use localhost or enable network providers", 403);
  }
  if (!url.pathname.endsWith("/")) url.pathname += "/";
  return url;
}

function redactedUrl(url) { return `${url.protocol}//${url.host}${url.pathname}`; }
async function providerError(response) { let detail = "Provider request failed"; try { const body = await response.json(); detail = body.error?.message || body.message || detail; } catch {} return new AppError("provider_error", detail, response.status >= 500 ? 503 : 422); }
function normalizeRequestError(error, signal) {
  if (signal?.aborted || error?.name === "AbortError") return new AppError("cancelled", "Generation cancelled", 499);
  return error instanceof AppError ? error : new AppError("provider_protocol_error", error?.message || "Provider request failed", 503);
}
