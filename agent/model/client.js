import http from "node:http";
import { AppError } from "../shared/errors.js";

export class ModelClient {
  constructor(host, port) { this.host = host; this.port = port; }

  request(path, payload = undefined, { signal } = {}) {
    return new Promise((resolve, reject) => {
      const body = payload === undefined ? null : JSON.stringify(payload);
      const req = http.request({ hostname: this.host, port: this.port, path, method: body ? "POST" : "GET", headers: body ? { "content-type": "application/json", "content-length": Buffer.byteLength(body) } : {} }, (res) => {
        let output = "";
        res.on("data", (chunk) => { output += chunk; });
        res.on("end", () => {
          try {
            const parsed = JSON.parse(output);
            if ((res.statusCode || 500) >= 400) reject(new AppError("model_error", parsed.error?.message || "Model runtime failed", 503));
            else resolve(parsed);
          } catch { reject(new AppError("model_protocol_error", "Model runtime returned invalid JSON", 503)); }
        });
      });
      req.on("error", () => reject(new AppError("model_unavailable", "MiniMind runtime is not available", 503)));
      if (signal) {
        if (signal.aborted) req.destroy(new Error("cancelled"));
        else signal.addEventListener("abort", () => req.destroy(new Error("cancelled")), { once: true });
      }
      req.setTimeout(120000, () => req.destroy(new Error("timeout")));
      if (body) req.write(body);
      req.end();
    });
  }

  health() { return this.request("/health"); }
  generate(payload, options) { return this.request("/generate", payload, options); }

  generateStream(payload, { signal, onEvent } = {}) {
    return new Promise((resolve, reject) => {
      const body = JSON.stringify(payload);
      const req = http.request({ hostname: this.host, port: this.port, path: "/generate/stream", method: "POST", headers: { "content-type": "application/json", "content-length": Buffer.byteLength(body), "accept": "application/x-ndjson" } }, (res) => {
        if ((res.statusCode || 500) >= 400) return reject(new AppError("model_error", "Model streaming failed", 503));
        let buffer = "";
        let final = null;
        const consume = (line) => {
          if (!line.trim()) return;
          const event = JSON.parse(line);
          if (event.type === "complete") final = event.result;
          else if (event.type === "cancelled") final = { type: "cancelled", content: "", tool: null };
          else onEvent?.(event);
        };
        res.setEncoding("utf8");
        res.on("data", (chunk) => {
          buffer += chunk;
          let index;
          while ((index = buffer.indexOf("\n")) >= 0) { consume(buffer.slice(0, index)); buffer = buffer.slice(index + 1); }
        });
        res.on("end", () => {
          try { if (buffer.trim()) consume(buffer); resolve(final || { type: "error", content: "stream ended without a final response", tool: null }); }
          catch { reject(new AppError("model_protocol_error", "Model stream returned invalid NDJSON", 503)); }
        });
      });
      req.on("error", (error) => reject(signal?.aborted ? new AppError("cancelled", "Generation cancelled", 499) : new AppError("model_unavailable", error.message, 503)));
      if (signal) {
        if (signal.aborted) req.destroy(new Error("cancelled"));
        else signal.addEventListener("abort", () => req.destroy(new Error("cancelled")), { once: true });
      }
      req.setTimeout(210000, () => req.destroy(new Error("timeout")));
      req.write(body); req.end();
    });
  }
}
