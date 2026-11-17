import { applyChanges, modelPayload, parseJsonChanges, promptWithSnapshot, resultText } from "../common.js";

export async function runBare({ item, root, provider }) {
  const contract = item.kind === "action"
    ? "不要调用工具。只返回 JSON：{\"changes\":[{\"path\":\"相对路径\",\"content\":\"完整文件内容\"}]}。"
    : "不要调用工具，只根据快照直接回答；没有证据时明确说明。";
  const payload = modelPayload(item, "bare", root, {
    mode: "chat",
    workspace: { root, files: [] },
    messages: [{ role: "system", content: "你是同模型裸提示基线。" }, { role: "user", content: promptWithSnapshot(item, root, contract) }],
  });
  const result = await provider.generate(payload);
  const content = resultText(result);
  const changes = item.kind === "action" ? parseJsonChanges(content) : [];
  const writtenPaths = changes.length ? applyChanges(root, changes) : [];
  return { content, writtenPaths, toolCalls: 0, activatedAgents: 0, events: [] };
}
