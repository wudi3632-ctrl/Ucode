import { StatelessExecutor } from "../../../agent/executor/stateless.js";
import { applyChanges, modelPayload, promptWithSnapshot, tools } from "../common.js";
import { indexFixture } from "../fixtures.js";

async function scout(item, root, provider, role) {
  const prompt = role === "structure"
    ? "只读定位与用户任务最相关的入口、实现文件和符号；不得猜测。"
    : "只读定位相关测试、风险和验证方法；不得提出写入。";
  const result = await provider.generate(modelPayload(item, `scout-${role}`, root, {
    mode: "project_read",
    tools: [],
    messages: [{ role: "system", content: prompt }, { role: "user", content: promptWithSnapshot(item, root) }],
  }));
  return result?.type === "final" ? String(result.content || "") : "未获得可用只读发现。";
}

function requestFor(item, root, group, scoutFindings = "") {
  const files = indexFixture(root);
  const action = item.kind === "action";
  const threadKey = `benchmark:${item.id}`;
  return {
    version: "ucode.executor.v1",
    requestId: `${threadKey}:${group}:request`,
    turnId: `benchmark:${item.id}:${group}`,
    threadKey,
    mode: item.kind,
    assignment: action ? { ticketId: `${item.id}-ticket`, assignmentId: `${item.id}-assignment` } : null,
    mandate: action ? {
      policyVersion: "benchmark-v1",
      projectId: item.id,
      threadKey,
      projectFingerprint: `fixture:${item.id}`,
      capabilities: ["read", "search", "test", "propose_write"],
    } : null,
    inputs: [{ id: `${item.id}-input`, kind: "trigger", triggerId: `${item.id}-trigger`, text: item.prompt, human: true }],
    history: [],
    workspace: { canonicalRoot: root, fingerprint: `fixture:${item.id}`, files },
    attachmentToken: null,
    attachments: [],
    metadata: scoutFindings ? { taskContext: { objective: item.prompt, scoutFindings } } : {},
  };
}

export async function runUCode({ item, root, provider, team = false }) {
  const shouldScout = team && item.teamEligible === true;
  const findings = shouldScout ? await Promise.all([scout(item, root, provider, "structure"), scout(item, root, provider, "verification")]) : [];
  const projectTools = tools();
  const executor = new StatelessExecutor({ tools: projectTools, coder: () => provider, controller: provider });
  const events = [];
  const content = await executor.execute(requestFor(item, root, team ? "ucode-team" : "ucode-single", findings.join("\n\n")), {
    onEvent: (event) => events.push(event),
  });
  const writtenPaths = [];
  for (const event of events) {
    if (event.type !== "effect_proposal" || !event.payload) continue;
    const changes = Array.isArray(event.payload.changes)
      ? event.payload.changes.filter((change) => change.operation !== "delete")
      : [{ path: event.payload.path, content: event.payload.content }];
    writtenPaths.push(...applyChanges(root, changes));
  }
  return {
    content: String(content || ""),
    writtenPaths,
    toolCalls: events.filter((event) => event.type === "activity" && ["read_file", "search", "pytest"].includes(event.kind)).length,
    activatedAgents: findings.length,
    events,
  };
}
