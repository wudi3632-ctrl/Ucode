import { TOOL_DEFINITIONS } from "../../../agent/tools/definitions.js";
import { validateToolRequest } from "../../../agent/executor/router.js";
import { applyChanges, modelPayload, promptWithSnapshot, resultText, tools } from "../common.js";
import { indexFixture } from "../fixtures.js";

export async function runReact({ item, root, provider }) {
  const projectTools = tools();
  const allowed = indexFixture(root).map((file) => file.path);
  const messages = [
    { role: "system", content: "你是通用 ReAct 编码 Agent。使用工具收集证据并完成任务；禁止访问项目外路径；完成后给出简短结论。" },
    { role: "user", content: `${item.prompt}\n\n项目文件索引：${allowed.join(", ")}` },
  ];
  let content = "";
  let toolCalls = 0;
  const writtenPaths = [];
  const events = [];
  for (let step = 0; step < 4; step += 1) {
    const result = await provider.generate(modelPayload(item, "react", root, { messages, tools: TOOL_DEFINITIONS, mode: "action" }));
    if (result?.type !== "tool_request") {
      content = resultText(result);
      break;
    }
    toolCalls += 1;
    let tool;
    try {
      tool = validateToolRequest(result.tool, allowed);
      events.push({ type: "tool", name: tool.name, status: "accepted" });
    } catch (error) {
      content = `安全停止：${error.message}`;
      events.push({ type: "tool", name: result.tool?.name || "unknown", status: "rejected", error: error.message });
      break;
    }
    if (tool.name === "write_file") {
      writtenPaths.push(...applyChanges(root, [tool.arguments]));
      content = `已修改 ${writtenPaths.join(", ")}。`;
      break;
    }
    if (tool.name === "write_files") {
      writtenPaths.push(...applyChanges(root, tool.arguments.changes.filter((change) => change.operation !== "delete")));
      content = `已修改 ${writtenPaths.join(", ")}。`;
      break;
    }
    let value;
    if (tool.name === "read_file") value = projectTools.readFile(root, tool.arguments);
    else if (tool.name === "search") value = projectTools.search(root, tool.arguments);
    else if (tool.name === "pytest") value = await projectTools.pytest(root, tool.arguments);
    else value = { error: "unsupported read-only tool" };
    messages.push({ role: "assistant", content: "", tool_calls: [{ name: tool.name, arguments: tool.arguments }] });
    messages.push({ role: "tool", content: JSON.stringify(value) });
  }
  if (!content) content = "达到 ReAct 步数上限，未声称完成。";
  return { content, writtenPaths, toolCalls, activatedAgents: 0, events };
}
