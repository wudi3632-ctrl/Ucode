import fs from "node:fs";
import path from "node:path";
import { ProjectTools } from "../../agent/tools/tools.js";
import { indexFixture, snapshotFixture } from "./fixtures.js";

export function tools() {
  return new ProjectTools({ timeoutMs: 20_000, maxOutputBytes: 40_000 });
}

export function modelPayload(item, group, root, extra = {}) {
  return {
    task_id: `benchmark:${item.id}:${group}`,
    messages: extra.messages || [{ role: "user", content: item.prompt }],
    workspace: { root, files: indexFixture(root) },
    tools: extra.tools || [],
    mode: extra.mode || item.kind,
    ...extra,
  };
}

export function promptWithSnapshot(item, root, contract = "") {
  return `${item.prompt}\n\n真实项目快照：${snapshotFixture(root)}\n\n${contract}`.trim();
}

export function resultText(result) {
  return result?.type === "final" ? String(result.content || "") : "";
}

export function applyChanges(root, changes) {
  const projectTools = tools();
  const normalized = changes.map((change) => ({
    path: String(change.path || "").replaceAll("\\", "/"),
    operation: fs.existsSync(path.join(root, ...String(change.path || "").split("/"))) ? "modify" : "create",
    content: String(change.content ?? ""),
  }));
  const effect = projectTools.prepareBatch(root, { changes: normalized });
  projectTools.applyBatch(root, effect);
  return normalized.map((item) => item.path);
}

export function parseJsonChanges(content) {
  const match = String(content || "").match(/\{[\s\S]*\}/);
  if (!match) return [];
  try {
    const value = JSON.parse(match[0]);
    return Array.isArray(value.changes) ? value.changes.filter((item) => item && typeof item.path === "string" && typeof item.content === "string") : [];
  } catch {
    return [];
  }
}
