import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { chooseContextAttachments } from "../bridge/desktop";
import type { ContextAttachmentSelection, EvidenceReceipt, RuntimeEvent, TicketSummary } from "../domain/types";
import { buildTimeline, Conversation } from "./Conversation";

vi.mock("../bridge/desktop", async (importOriginal) => ({
  ...await importOriginal<typeof import("../bridge/desktop")>(),
  chooseContextAttachments: vi.fn(async () => null),
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

const ticket: TicketSummary = {
  id: "ticket-1",
  projectId: "project-1",
  title: "真实对话",
  request: "你好",
  state: "completed",
  createdAt: "2026-11-10T00:00:00Z",
  updatedAt: "2026-11-10T00:00:01Z",
  parentTicketId: null,
  branchEventSequence: null,
  pinned: false,
  archivedAt: null,
};

function event(sequence: number, eventType: string, payload: Record<string, unknown> = {}): RuntimeEvent {
  return { sequence, eventType, payload, ticketId: ticket.id, createdAt: "2026-11-10T00:00:00Z" };
}

describe("conversation timeline projection", () => {
  it("groups Rescue probe evidence as factual activity before the report", () => {
    const items = buildTimeline(ticket, [
      event(1, "rescue.probe.completed", { toolId: "network.adapter.inspect", summary: "发现 1 个启用的 IP 配置", evidenceId: "evidence-1" }),
      event(2, "rescue.diagnosis.ready", { outcome: "safe_stop", mutationPerformed: false }),
      event(3, "delivery.ready", { content: "仍需经过授权的 DNS 探测；本轮没有更改系统。", route: "rescue" }),
    ], false);
    expect(items.map((item) => item.kind)).toEqual(["user", "activity", "assistant"]);
    const activity = items[1];
    expect(activity.kind === "activity" ? activity.events.map((item) => item.eventType) : []).toEqual([
      "rescue.probe.completed",
      "rescue.diagnosis.ready",
    ]);
  });

  it("describes a Rescue receipt as system evidence with no system mutation", () => {
    const receipt: EvidenceReceipt = {
      ticketId: ticket.id,
      request: "Windows 无法上网，请诊断",
      route: "rescue",
      outcome: "waiting_input",
      sources: [{ toolName: "network.adapter.inspect", evidenceId: "evidence-1", status: "observed", preview: "发现 1 个启用的 IP 配置" }],
      commands: [], tests: [], effects: [],
      decisions: [{ event: "rescue.diagnosis.ready", detail: { outcome: "safe_stop", mutationPerformed: false } }],
      finalState: {}, contentDigest: "digest", updatedAt: "2026-11-10T00:00:01Z",
    };
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} effect={null} receipt={receipt} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    const card = screen.getByRole("article", { name: "结果证据" });
    expect(card).toHaveTextContent("没有修改系统");
    expect(card).toHaveTextContent("1 项系统只读证据已核验");
    expect(card).toHaveTextContent("未执行修复");
    expect(card).toHaveTextContent("无需回滚");
  });

  it("labels Rescue task context as system diagnosis instead of asking for project files", () => {
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[event(1, "context.updated", {
        scope: "system",
        objective: "Windows PATH 异常，请根据真实系统证据诊断",
        targetFiles: [], constraints: [], verification: [], unresolvedQuestions: [], revision: 2,
      })]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);

    expect(screen.getByText("系统诊断")).toBeInTheDocument();
    expect(screen.queryByText("尚未限定文件")).not.toBeInTheDocument();
    expect(screen.queryByText(/需要明确目标文件/)).not.toBeInTheDocument();
    expect(screen.queryByText(/需要明确验证方式/)).not.toBeInTheDocument();
  });

  it("reports verified Rescue writes and exact rollback instead of claiming no system change", () => {
    const verifiedReceipt: EvidenceReceipt = {
      ticketId: ticket.id,
      request: "规范化当前用户 PATH",
      route: "rescue",
      outcome: "waiting_input",
      sources: [{ toolName: "system.environment.inspect", evidenceId: "evidence-1", status: "observed" }],
      commands: [], tests: [], decisions: [],
      effects: [
        { event: "rescue.repair.proposed", detail: { proposal: { id: "repair-1" } } },
        { event: "rescue.repair.verified", detail: { record: { proposal: { rollbackContract: "仅当当前摘要未变化时恢复原值" } } } },
      ],
      finalState: {}, contentDigest: "digest", updatedAt: "2026-11-10T00:00:01Z",
    };
    const common = {
      projectId: "project-1", ticket, events: [], effect: null, appliedEffect: null, busy: false,
      onSubmit: vi.fn(), onCancel: vi.fn(), onRetry: vi.fn(), onExport: vi.fn(), onBranch: vi.fn(),
      onApprove: vi.fn(), onReject: vi.fn(), onRollback: vi.fn(), onExportReceipt: vi.fn(), hasProject: true, onOpenProject: vi.fn(), error: null,
      sidebarOpen: true, inspectorOpen: false, onToggleSidebar: vi.fn(), onToggleInspector: vi.fn(),
      providerStatus: "ready" as const, modelProvider: "bundled" as const, providerModel: "", modelPerformance: null,
    };
    const view = render(<Conversation {...common} receipt={verifiedReceipt} />);

    let card = screen.getByRole("article", { name: "结果证据" });
    expect(card).toHaveTextContent("写入已验证");
    expect(card).toHaveTextContent("已应用 1 个用户级系统修复");
    expect(card).toHaveTextContent("修复后已重读目标并核对精确摘要");
    expect(card).toHaveTextContent("仅当当前摘要未变化时恢复原值");
    expect(card).not.toHaveTextContent("没有修改系统");

    view.rerender(<Conversation {...common} receipt={{
      ...verifiedReceipt,
      effects: [...verifiedReceipt.effects, { event: "rescue.repair.rolled_back", detail: { status: "rolled_back" } }],
    }} />);
    card = screen.getByRole("article", { name: "结果证据" });
    expect(card).toHaveTextContent("用户级系统修改已精确回滚");
    expect(card).toHaveTextContent("回滚后已重读目标并确认恢复审批前原值");
    expect(card).toHaveTextContent("已完成回滚，无需再次撤销");
  });

  it("describes read-only evidence without implying that tests or Diff review were required", () => {
    const receipt: EvidenceReceipt = {
      ticketId: ticket.id,
      request: "calculator.py的内容",
      route: "project_read",
      outcome: "completed",
      sources: [{ path: "calculator.py" }],
      commands: [],
      tests: [],
      effects: [],
      decisions: [],
      finalState: {},
      contentDigest: "digest",
      updatedAt: "2026-11-10T00:00:01Z",
    };
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} effect={null} receipt={receipt} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);

    const evidence = screen.getByRole("article", { name: "结果证据" });
    expect(evidence).toHaveTextContent("calculator.py的内容");
    expect(evidence).toHaveTextContent("1 项项目来源已核验");
    expect(evidence).toHaveTextContent("回答范围仅限于本轮实际读取的项目内容");
    expect(evidence).not.toHaveTextContent("未记录测试结果");
    expect(evidence).not.toHaveTextContent("审查 Diff");
  });

  it("describes a rejected proposal as no write instead of a completed change", () => {
    const receipt: EvidenceReceipt = {
      ticketId: ticket.id,
      request: "创建 test_fibonacci.py",
      route: "explicit_batch_edit",
      outcome: "completed",
      sources: [], commands: [], tests: [],
      effects: [{ event: "effect.proposed", detail: { effectId: "effect-1" } }],
      decisions: [{ event: "effect.rejected", detail: { effectId: "effect-1" } }],
      finalState: {}, contentDigest: "digest", updatedAt: "2026-11-10T00:00:01Z",
    };
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} effect={null} receipt={receipt} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);

    const evidence = screen.getByRole("article", { name: "结果证据" });
    expect(evidence).toHaveTextContent("已取消");
    expect(evidence).toHaveTextContent("没有写入文件");
    expect(evidence).toHaveTextContent("修改已拒绝，项目文件未改变");
    expect(evidence).toHaveTextContent("没有需要撤销的文件批次");
    expect(evidence).not.toHaveTextContent("文件已变更");
  });

  it("describes chat without pretending project evidence was checked", () => {
    const receipt: EvidenceReceipt = {
      ticketId: ticket.id, request: "15", route: "chat", outcome: "completed",
      sources: [], commands: [], tests: [], effects: [], decisions: [], finalState: {},
      contentDigest: "digest", updatedAt: "2026-11-10T00:00:01Z",
    };
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} effect={null} receipt={receipt} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    const evidence = screen.getByRole("article", { name: "结果证据" });
    expect(evidence).toHaveTextContent("本轮请求15");
    expect(evidence).toHaveTextContent("本轮不需要项目证据");
    expect(evidence).toHaveTextContent("未读取项目文件；本回答不构成项目证据");
  });

  it("shows the observed pytest result instead of counting it as a generic source", () => {
    const receipt: EvidenceReceipt = {
      ticketId: ticket.id,
      request: "创建并验证 fibonacci.py",
      route: "verified_edit",
      outcome: "completed",
      sources: [], commands: [],
      tests: [{ toolName: "pytest", status: "succeeded", exitCode: 0, sourcePaths: ["test_fibonacci.py"], preview: "3 passed in 0.02s" }],
      effects: [{ event: "effect.applied", detail: { effectId: "effect-1" } }],
      decisions: [], finalState: {}, contentDigest: "digest", updatedAt: "2026-11-10T00:00:01Z",
    };
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} effect={null} receipt={receipt} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);

    const evidence = screen.getByRole("article", { name: "结果证据" });
    expect(evidence).toHaveTextContent("pytest：3 passed · test_fibonacci.py");
    expect(evidence).not.toHaveTextContent("项目来源已核验");
  });

  it("repairs legacy receipts that stored delivery pytest evidence under sources", () => {
    const receipt: EvidenceReceipt = {
      ticketId: ticket.id, request: "验证 fibonacci.py", route: "verified_edit", outcome: "completed",
      sources: [{ toolName: "pytest", status: "succeeded", exitCode: 0, sourcePaths: ["test_fibonacci.py"], preview: "3 passed in 0.02s" }],
      commands: [], tests: [], effects: [{ event: "effect.applied" }], decisions: [], finalState: {}, contentDigest: "digest", updatedAt: "2026-11-10T00:00:01Z",
    };
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} effect={null} receipt={receipt} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    const evidence = screen.getByRole("article", { name: "结果证据" });
    expect(evidence).toHaveTextContent("pytest：3 passed · test_fibonacci.py");
    expect(evidence).toHaveTextContent("来源 0");
    expect(evidence).toHaveTextContent("测试 1");
  });

  it("describes a no-winner race as failed and explains why pytest was skipped", () => {
    const receipt: EvidenceReceipt = {
      ticketId: ticket.id, request: "修改三个文件并运行 pytest", route: "candidate_race", outcome: "failed",
      sources: [], commands: [], tests: [], effects: [],
      decisions: [{ event: "race.decided", detail: {
        race: { verdict: { kind: "no_winner" } },
        testsExecuted: false,
        testSkippedReason: "两个候选均未完整覆盖任务目标，机械验收未执行，已在写入前安全停止。",
        candidates: [{ id: "candidate-a", facts: [{ outcome: "failed", summary: "遗漏 report.py" }] }],
      } }],
      finalState: { ticketState: "failed" }, contentDigest: "digest", updatedAt: "2026-11-10T00:00:01Z",
    };
    render(<Conversation
      projectId="project-1" ticket={{ ...ticket, state: "failed" }} events={[]} effect={null} receipt={receipt} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);

    const evidence = screen.getByRole("article", { name: "结果证据" });
    expect(evidence).toHaveTextContent("失败");
    expect(evidence).toHaveTextContent("机械验收未执行");
    expect(evidence).toHaveTextContent("没有写入文件");
    expect(evidence).toHaveTextContent("技术详情 · 1 条事实");
    expect(evidence).not.toHaveTextContent("本轮不需要运行测试");
  });

  it("labels tests from an invalidated legacy race as candidate-only evidence", () => {
    const receipt: EvidenceReceipt = {
      ticketId: ticket.id, request: "旧候选任务", route: "candidate_race", outcome: "failed",
      sources: [], commands: [], effects: [],
      tests: [{ toolName: "candidate_test", status: "passed", exitCode: 0, preview: "3 passed" }],
      decisions: [{ event: "race.legacy_stopped", detail: { reason: "legacy_protocol_missing_targets" } }],
      finalState: { ticketState: "failed" }, contentDigest: "digest", updatedAt: "2026-11-10T00:00:01Z",
    };
    render(<Conversation
      projectId="project-1" ticket={{ ...ticket, state: "failed" }} events={[]} effect={null} receipt={receipt} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    expect(screen.getByRole("article", { name: "结果证据" })).toHaveTextContent("仅旧候选验证：pytest：3 passed；任务已在写入前安全失效");
  });

  it("reveals observed response performance only inside the model menu", () => {
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel=""
      modelPerformance={{ outcome: "completed", active: false, requestId: "request-1", firstCharacterMs: 1200, charactersPerSecond: 5, characterCount: 5, startedAt: "2026-11-11T00:00:00Z", firstOutputAt: "2026-11-11T00:00:01.2Z", lastOutputAt: "2026-11-11T00:00:02.2Z", endedAt: "2026-11-11T00:00:02.4Z" }}
    />);
    expect(screen.queryByText(/首字 1\.2 秒/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /内置模型/ }));
    const menu = screen.getByRole("dialog", { name: "模型状态" });
    expect(menu).toHaveTextContent("最近响应 · 已完成");
    expect(menu).toHaveTextContent("首字 1.2 秒");
    expect(menu).toHaveTextContent("输出 5.0 字/秒");
    expect(menu).toHaveTextContent("5 字");
    expect(menu).toHaveTextContent("端到端观测，包含推理与本地传输");
  });

  it("labels active, cancelled, failed, and external-provider samples honestly", () => {
    const base = {
      projectId: "project-1", ticket, events: [], effect: null, receipt: null, appliedEffect: null, busy: false,
      onSubmit: vi.fn(), onCancel: vi.fn(), onRetry: vi.fn(), onExport: vi.fn(), onBranch: vi.fn(),
      onApprove: vi.fn(), onReject: vi.fn(), onRollback: vi.fn(), onExportReceipt: vi.fn(), hasProject: true, onOpenProject: vi.fn(), error: null,
      sidebarOpen: true, inspectorOpen: false, onToggleSidebar: vi.fn(), onToggleInspector: vi.fn(),
      providerStatus: "ready" as const, modelProvider: "bundled" as const, providerModel: "",
    };
    const view = render(<Conversation {...base} modelPerformance={{ outcome: "generating", active: true, requestId: "active", firstCharacterMs: null, charactersPerSecond: null, characterCount: 0, startedAt: "2026-11-11T00:00:00Z", firstOutputAt: null, lastOutputAt: null, endedAt: null }} />);
    fireEvent.click(screen.getByRole("button", { name: /内置模型/ }));
    expect(screen.getByLabelText("最近模型性能")).toHaveTextContent("最近响应 · 生成中正在等待首字…");

    view.rerender(<Conversation {...base} modelPerformance={{ outcome: "cancelled", active: false, requestId: "cancelled", firstCharacterMs: 500, charactersPerSecond: 4.2, characterCount: 6, startedAt: "2026-11-11T00:00:00Z", firstOutputAt: "2026-11-11T00:00:00.5Z", lastOutputAt: "2026-11-11T00:00:01Z", endedAt: "2026-11-11T00:00:01.1Z" }} />);
    expect(screen.getByLabelText("最近模型性能")).toHaveTextContent("最近响应 · 已停止首字 500 毫秒输出 4.2 字/秒6 字");

    view.rerender(<Conversation {...base} modelPerformance={{ outcome: "failed", active: false, requestId: "failed", firstCharacterMs: null, charactersPerSecond: null, characterCount: 0, startedAt: "bad", firstOutputAt: null, lastOutputAt: null, endedAt: "2026-11-11T00:00:01Z" }} />);
    expect(screen.getByLabelText("最近模型性能")).toHaveTextContent("最近响应 · 失败尚无可测量的回答");

    view.rerender(<Conversation {...base} modelProvider="openai-compatible" providerModel="qwen-local" modelPerformance={null} />);
    expect(screen.getByRole("dialog", { name: "模型状态" })).toHaveTextContent("qwen-local");
    expect(screen.getByLabelText("最近模型性能")).toHaveTextContent("最近响应尚无可测量的回答");
  });

  it("offers an explicit safe recovery action when the local runtime is unavailable", () => {
    const onRecoverProvider = vi.fn().mockResolvedValue(undefined);
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="unavailable" modelProvider="bundled" providerModel="" modelPerformance={null}
      providerRecovering={false} onRecoverProvider={onRecoverProvider}
    />);

    expect(screen.getByText("本地模型服务未响应")).toBeInTheDocument();
    expect(screen.getByLabelText("任务输入")).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "恢复本地服务" }));
    expect(onRecoverProvider).toHaveBeenCalledOnce();
  });

  it("shows ordinary chat without fake thought or transport activity", () => {
    const items = buildTimeline(ticket, [
      event(1, "provider.stream.started"),
      event(2, "route.selected", { intent: "chat" }),
      event(3, "provider.delta", { requestId: "request-1", delta: "你" }),
      event(4, "provider.delta", { requestId: "request-1", delta: "好" }),
      event(5, "delivery.ready", { content: "你好", route: "chat" }),
    ], false);

    expect(items.map((item) => item.kind)).toEqual(["user", "assistant"]);
    expect(items[1]).toMatchObject({ kind: "assistant", content: "你好", streaming: false });
  });

  it("groups only factual project operations into an activity item", () => {
    const items = buildTimeline(ticket, [
      event(1, "provider.started"),
      event(2, "tool.completed", { tool: "read_file", evidence: { path: "README.md" } }),
      event(3, "delivery.ready", { content: "项目说明", route: "project_read" }),
    ], false);

    expect(items.map((item) => item.kind)).toEqual(["user", "activity", "assistant"]);
  });

  it("renders structured no-op rescue actions without inferring them from prose", () => {
    const onOpenSettings = vi.fn();
    const events = [event(1, "delivery.ready", {
      content: "本地模型没有生成实际内容变化。",
      route: "failed_test_repair",
      recovery: {
        reason: "candidate_no_change",
        path: "ucodebench/cart.py",
        attempts: 2,
        actions: ["attach_context", "configure_model"],
      },
    })];
    expect(buildTimeline(ticket, events, false).map((item) => item.kind)).toEqual(["user", "assistant", "rescue"]);
    render(<Conversation
      projectId="project-1" ticket={{ ...ticket, state: "waiting" }} events={events} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} onOpenSettings={onOpenSettings} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);

    expect(screen.getByLabelText("任务继续选项")).toHaveTextContent("ucodebench/cart.py · 2 次候选无有效实现变化");
    fireEvent.click(screen.getByRole("button", { name: "补充上下文" }));
    expect(chooseContextAttachments).toHaveBeenCalledWith("project-1");
    fireEvent.click(screen.getByRole("button", { name: "调整模型" }));
    expect(onOpenSettings).toHaveBeenCalledOnce();
  });

  it("does not show rescue actions for ordinary clarification", () => {
    const items = buildTimeline(ticket, [event(1, "delivery.ready", { content: "请指定要读取的文件。", route: "read_file" })], false);
    expect(items.map((item) => item.kind)).toEqual(["user", "assistant"]);
  });

  it("persists a recovery card, hides reapproval, and locks further input", () => {
    const events = [
      event(1, "effect.proposed", { id: "effect-recovery", changes: [
        { path: "src/a.py", operation: "modify", before: "old", after: "new" },
        { path: "src/b.py", operation: "delete", before: "old", after: "" },
      ] }),
      event(2, "effect.recovery_required", { effectId: "effect-recovery", message: "automatic recovery could not be verified" }),
    ];
    expect(buildTimeline({ ...ticket, state: "failed" }, events, false).map((item) => item.kind)).toEqual(["user", "recovery"]);
    render(<Conversation
      projectId="project-1" ticket={{ ...ticket, state: "failed" }} events={events} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error="批量写入失败"
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel=""
      modelPerformance={null}
    />);
    expect(screen.getByRole("alert")).toHaveTextContent("工作区需要人工恢复");
    expect(screen.getByLabelText("需要检查的文件")).toHaveTextContent("src/a.py");
    expect(screen.queryByRole("button", { name: /批准选中的/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "重试本轮" })).not.toBeInTheDocument();
    expect(screen.getByLabelText("任务输入")).toBeDisabled();
  });

  it("deduplicates final delivery and never revives an old partial stream", () => {
    const items = buildTimeline(ticket, [
      event(1, "provider.delta", { requestId: "old", delta: "旧的未完成回答" }),
      event(2, "user.message", { content: "新的问题", clientRequestId: "request-new" }),
      event(3, "provider.message", { content: "新的回答" }),
      event(4, "delivery.ready", { content: "新的回答" }),
      event(5, "user.message", { content: "新的问题", clientRequestId: "request-new" }),
    ], true);

    const assistants = items.filter((item) => item.kind === "assistant");
    expect(assistants).toHaveLength(2);
    expect(assistants[0]).toMatchObject({ content: "旧的未完成回答", streaming: false });
    expect(assistants[1]).toMatchObject({ content: "新的回答", streaming: false });
    expect(items.filter((item) => item.kind === "user" && item.content === "新的问题")).toHaveLength(1);
  });

  it("never renders streamed raw tool JSON when the provider rejects it", () => {
    const items = buildTimeline({ ...ticket, state: "failed" }, [
      event(1, "provider.delta", { requestId: "bad-tool", delta: "[{\"name\": \"read_file\"," }),
      event(2, "provider.delta", { requestId: "bad-tool", delta: " \"arguments\": {\"path\": \"src/a.py\"}}]" }),
      event(3, "provider.error", { error: { code: "chat_protocol_error" } }),
      event(4, "ticket.failed", { message: "本地模型没有生成可用的项目回答。" }),
    ], false);

    expect(items.filter((item) => item.kind === "assistant")).toHaveLength(0);
    expect(JSON.stringify(items)).not.toContain("read_file");
    expect(JSON.stringify(items)).not.toContain("arguments");
  });

  it("locks submission synchronously and clears the accepted draft immediately", () => {
    let finish: ((accepted: boolean) => void) | undefined;
    const onSubmit = vi.fn(() => new Promise<boolean>((resolve) => { finish = resolve; }));
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={onSubmit} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel=""
      modelPerformance={null}
    />);
    const input = screen.getByLabelText("任务输入");
    fireEvent.change(input, { target: { value: "只应发送一次" } });
    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(input).toHaveValue("");
    finish?.(true);
  });

  it("navigates current-conversation input history with a collapsed end caret and restores the original draft", async () => {
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[event(1, "user.message", { content: "第二条问题" })]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    const input = screen.getByLabelText("任务输入") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "尚未发送的草稿" } });
    input.setSelectionRange(0, 0);

    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(input).toHaveValue("第二条问题");
    await waitFor(() => {
      expect(input.selectionStart).toBe("第二条问题".length);
      expect(input.selectionEnd).toBe("第二条问题".length);
    });
    expect(screen.getByRole("status")).toHaveTextContent("历史 2/2");
    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(input).toHaveValue("你好");
    await waitFor(() => {
      expect(input.selectionStart).toBe("你好".length);
      expect(input.selectionEnd).toBe("你好".length);
    });
    expect(screen.getByRole("status")).toHaveTextContent("历史 1/2");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(input).toHaveValue("第二条问题");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(input).toHaveValue("尚未发送的草稿");
    await waitFor(() => {
      expect(input.selectionStart).toBe("尚未发送的草稿".length);
      expect(input.selectionEnd).toBe("尚未发送的草稿".length);
    });
    expect(screen.queryByText(/历史 \d+\/\d+/)).not.toBeInTheDocument();

    input.setSelectionRange(0, 0);
    fireEvent.keyDown(input, { key: "ArrowUp" });
    fireEvent.keyDown(input, { key: "Escape" });
    expect(input).toHaveValue("尚未发送的草稿");
    await waitFor(() => {
      expect(input.selectionStart).toBe("尚未发送的草稿".length);
      expect(input.selectionEnd).toBe("尚未发送的草稿".length);
    });
  });

  it("leaves multiline and IME direction keys to the native editor", () => {
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    const input = screen.getByLabelText("任务输入") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "第一行\n第二行" } });
    input.setSelectionRange(4, 4);
    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(input).toHaveValue("第一行\n第二行");
    expect(screen.queryByText(/历史 \d+\/\d+/)).not.toBeInTheDocument();

    fireEvent.change(input, { target: { value: "" } });
    fireEvent.keyDown(input, { key: "ArrowUp", isComposing: true });
    expect(input).toHaveValue("");
  });

  it("keeps selected attachments while recalling history and explains attachment limits", async () => {
    const selected: ContextAttachmentSelection = {
      selectionId: "selection-test",
      attachments: [{ id: "attachment-test", displayName: "notes.md", source: "project", relativePath: "notes.md", sizeBytes: 20, includedBytes: 20, truncated: false, sha256: "a".repeat(64) }],
      errors: [],
    };
    vi.mocked(chooseContextAttachments).mockResolvedValueOnce(selected);
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    const add = screen.getByRole("button", { name: "添加上下文文件" });
    expect(add).toHaveAttribute("title", "添加上下文文件（最多 8 个 UTF-8 文本）");
    fireEvent.click(add);
    expect(await screen.findByLabelText("已添加上下文")).toHaveTextContent("notes.md");

    const input = screen.getByLabelText("任务输入") as HTMLTextAreaElement;
    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(input).toHaveValue("你好");
    expect(screen.getByLabelText("已添加上下文")).toHaveTextContent("notes.md");
  });

  it("appends files to the current attachment selection instead of replacing it", async () => {
    const first: ContextAttachmentSelection = {
      selectionId: "selection-test",
      attachments: [{ id: "attachment-first", displayName: "inside.md", source: "project", relativePath: "inside.md", sizeBytes: 10, includedBytes: 10, truncated: false, sha256: "a".repeat(64) }],
      errors: [],
    };
    const second: ContextAttachmentSelection = {
      selectionId: "selection-test",
      attachments: [
        ...first.attachments,
        { id: "attachment-second", displayName: "outside.txt", source: "external_readonly", relativePath: null, sizeBytes: 12, includedBytes: 12, truncated: false, sha256: "b".repeat(64) },
      ],
      errors: [],
    };
    vi.mocked(chooseContextAttachments)
      .mockResolvedValueOnce(first)
      .mockResolvedValueOnce(second);
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);

    fireEvent.click(screen.getByRole("button", { name: "添加上下文文件" }));
    expect(await screen.findByText("inside.md")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "添加上下文文件" }));
    expect(await screen.findByText("outside.txt")).toBeInTheDocument();
    expect(screen.getByLabelText("已添加上下文")).toHaveTextContent("inside.md");
    expect(chooseContextAttachments).toHaveBeenNthCalledWith(1, "project-1");
    expect(chooseContextAttachments).toHaveBeenNthCalledWith(2, "project-1", "selection-test");
  });

  it("approves only the explicitly selected files from a visible batch", () => {
    const onApprove = vi.fn();
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} receipt={null} appliedEffect={null} busy={false}
      effect={{ id: "effect-batch", risk: "write", changes: [
        { path: "src/main.ts", operation: "modify", before: "old", after: "new", beforeDigest: "a", afterDigest: "b" },
        { path: "docs/new.md", operation: "create", before: "", after: "# New", beforeDigest: null, afterDigest: "c" },
      ] }}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={onApprove} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel=""
      modelPerformance={null}
    />);

    expect(screen.getByText("2 个文件变更")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: /docs\/new\.md/ }));
    fireEvent.click(screen.getByRole("button", { name: "批准选中的 1 个文件" }));
    expect(onApprove).toHaveBeenCalledWith(["src/main.ts"]);
  });

  it("shows actual changed-line counts instead of total file lengths", () => {
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} receipt={null} appliedEffect={null} busy={false}
      effect={{ id: "effect-lines", risk: "write", changes: [
        { path: "src/calc.ts", operation: "modify", before: "one\ntwo\nthree\nfour", after: "one\ntwo changed\nthree\nfour", beforeDigest: "a", afterDigest: "b" },
      ] }}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);

    const row = screen.getByRole("checkbox", { name: /src\/calc\.ts/ }).closest("label");
    expect(row).toHaveTextContent("+1");
    expect(row).toHaveTextContent("−1");
    expect(row).not.toHaveTextContent("+4");
    expect(row).not.toHaveTextContent("−4");
  });

  it("requires an explicit decision for an evidence-bound R2 repair and exposes verified rollback", async () => {
    const proposal = {
      schemaVersion: 1, id: "repair-1", ticketId: ticket.id, operationId: "system.user_path.normalize", operationVersion: 1, risk: "r2",
      target: "HKCU\\Environment\\Path", normalizedArguments: { after: { value: "C:\\Windows" }, removedEntries: ["C:\\Windows（重复）"] }, evidenceIds: ["evidence-1"],
      prerequisites: ["摘要一致"], snapshotPlan: "保存原值", expectedPostcondition: "PATH 已规范化", verificationProbe: "重新读取用户 PATH", rollbackContract: "当前值未变化时恢复",
      beforeDigest: "a".repeat(64), expectedAfterDigest: "b".repeat(64), nonce: "nonce", createdAt: "2026-11-10T00:00:00Z", expiresAt: "2026-11-10T00:10:00Z", status: "awaiting_approval", proposalDigest: "c".repeat(64),
    };
    const onResolve = vi.fn(async () => undefined);
    const view = render(<Conversation
      projectId="project-1" ticket={ticket} events={[event(1, "rescue.repair.proposed", { proposal })]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()} onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()}
      onResolveRescueRepair={onResolve} onRollbackRescueRepair={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()} providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    const card = screen.getByRole("article", { name: "Rescue 可逆修复" });
    expect(card).toHaveTextContent("模型无权执行");
    expect(card).toHaveTextContent("aaaaaaaaaa");
    fireEvent.click(within(card).getByRole("button", { name: "批准、应用并验证" }));
    await waitFor(() => expect(onResolve).toHaveBeenCalledWith("repair-1", true));

    const verified = { ...proposal, status: "verified" };
    view.rerender(<Conversation
      projectId="project-1" ticket={ticket} events={[event(1, "rescue.repair.proposed", { proposal }), event(2, "rescue.repair.verified", { proposalId: "repair-1", record: { proposal: verified, snapshot: {}, detail: "修复已通过目标重读验证", updatedAt: "2026-11-10T00:00:02Z" } })]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()} onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()}
      onResolveRescueRepair={onResolve} onRollbackRescueRepair={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()} providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    expect(screen.getByRole("button", { name: "回滚本次系统修改" })).toBeInTheDocument();
  });

  it("labels an R3 machine PATH repair as a one-time UAC operation", async () => {
    const proposal = {
      schemaVersion: 1, id: "repair-r3", ticketId: ticket.id, operationId: "system.machine_path.normalize", operationVersion: 1, risk: "r3",
      target: "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment\\Path", normalizedArguments: { after: { value: "C:\\Windows" }, removedEntries: ["C:\\Windows（重复）"] }, evidenceIds: ["evidence-r3"],
      prerequisites: ["摘要一致", "UAC 明确批准"], snapshotPlan: "保存原值", expectedPostcondition: "机器 PATH 已规范化", verificationProbe: "helper 与宿主双重重读", rollbackContract: "第二次 UAC 后恢复",
      beforeDigest: "d".repeat(64), expectedAfterDigest: "e".repeat(64), nonce: "nonce-r3", createdAt: "2026-11-10T00:00:00Z", expiresAt: "2026-11-10T00:10:00Z", status: "awaiting_approval", proposalDigest: "f".repeat(64),
    };
    const onResolve = vi.fn(async () => undefined);
    const onRollback = vi.fn(async () => undefined);
    const view = render(<Conversation
      projectId="project-1" ticket={ticket} events={[event(1, "rescue.repair.proposed", { proposal })]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()} onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()}
      onResolveRescueRepair={onResolve} onRollbackRescueRepair={onRollback} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()} providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    const card = screen.getByRole("article", { name: "Rescue 可逆修复" });
    expect(card).toHaveTextContent("R3");
    expect(card).toHaveTextContent("规范化机器 PATH");
    expect(card).toHaveTextContent("需要 Windows UAC 管理员确认");
    expect(card).toHaveTextContent("短生命周期白名单 helper");
    fireEvent.click(within(card).getByRole("button", { name: "批准并打开 UAC" }));
    await waitFor(() => expect(onResolve).toHaveBeenCalledWith("repair-r3", true));

    const unverified = { ...proposal, status: "repaired_unverified" };
    view.rerender(<Conversation
      projectId="project-1" ticket={ticket} events={[event(1, "rescue.repair.proposed", { proposal }), event(2, "rescue.repair.unverified", { proposalId: "repair-r3", record: { proposal: unverified, snapshot: {}, detail: "helper 响应丢失，但目标摘要已观察到", updatedAt: "2026-11-10T00:00:02Z" } })]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()} onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()}
      onResolveRescueRepair={onResolve} onRollbackRescueRepair={onRollback} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()} providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    expect(screen.getByText("写入状态未完全验证", { exact: false })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "按快照安全回滚" }));
    expect(screen.getByText("回滚会再次打开 UAC。", { exact: false })).toBeInTheDocument();

    const cancelled = { ...proposal, status: "uac_cancelled" };
    view.rerender(<Conversation
      projectId="project-1" ticket={ticket} events={[event(1, "rescue.repair.proposed", { proposal }), event(3, "rescue.repair.safe_stop", { proposalId: "repair-r3", record: { proposal: cancelled, snapshot: {}, detail: "Windows UAC 未获批准；机器 PATH 没有变化", updatedAt: "2026-11-10T00:00:03Z" } })]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()} onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()}
      onResolveRescueRepair={onResolve} onRollbackRescueRepair={onRollback} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()} providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    expect(screen.getByText("UAC 未批准，未修改", { exact: false })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "批准并打开 UAC" })).not.toBeInTheDocument();
  });

  it("shows a bounded R2 manual-proxy repair without exposing a proxy endpoint", async () => {
    const proposal = {
      schemaVersion: 1, id: "repair-proxy", ticketId: ticket.id, operationId: "network.user_proxy.disable_manual", operationVersion: 1, risk: "r2",
      target: "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\\ProxyEnable", normalizedArguments: { after: { proxyEnablePresent: true, proxyEnable: 0 }, changeSummary: "仅禁用手动代理开关" }, evidenceIds: ["evidence-proxy"],
      prerequisites: ["代理探针已观察"], snapshotPlan: "保存 DWORD 原值", expectedPostcondition: "手动代理开关为禁用", verificationProbe: "重读 ProxyEnable", rollbackContract: "摘要一致时恢复原值",
      beforeDigest: "1".repeat(64), expectedAfterDigest: "2".repeat(64), nonce: "nonce-proxy", createdAt: "2026-11-10T00:00:00Z", expiresAt: "2026-11-10T00:10:00Z", status: "awaiting_approval", proposalDigest: "3".repeat(64),
    };
    const onResolve = vi.fn(async () => undefined);
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[event(1, "rescue.repair.proposed", { proposal })]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()} onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()}
      onResolveRescueRepair={onResolve} onRollbackRescueRepair={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()} providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    const card = screen.getByRole("article", { name: "Rescue 可逆修复" });
    expect(card).toHaveTextContent("禁用当前用户手动代理");
    expect(card).toHaveTextContent("不删除代理服务器地址、绕过列表或 PAC 配置");
    expect(card).toHaveTextContent("ProxyEnable");
    expect(card).not.toHaveTextContent("proxy.example");
    fireEvent.click(within(card).getByRole("button", { name: "批准、应用并验证" }));
    await waitFor(() => expect(onResolve).toHaveBeenCalledWith("repair-proxy", true));
  });

  it("shows an exact one-time R1 network boundary and records the observed result", async () => {
    const proposal = {
      schemaVersion: 1, id: "probe-1", ticketId: ticket.id, toolId: "network.tcp.probe", toolVersion: 1, risk: "r1",
      target: "example.com:443", normalizedArguments: { host: "example.com", port: 443 }, evidenceIds: ["evidence-r0"],
      purpose: "确认目标端口是否可达", dataBoundary: "仅向 example.com 的 443 端口发起 TCP 握手；不发送 HTTP 或应用数据", timeoutMs: 12000,
      nonce: "nonce", createdAt: "2026-11-10T00:00:00Z", expiresAt: "2026-11-10T00:10:00Z", status: "awaiting_approval", proposalDigest: "a".repeat(64),
    };
    const onResolve = vi.fn(async () => undefined);
    const view = render(<Conversation
      projectId="project-1" ticket={ticket} events={[event(1, "rescue.network_probe.proposed", { proposal })]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()} onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()}
      onResolveRescueProbe={onResolve} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()} providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    const card = screen.getByRole("article", { name: "Rescue 联网探测" });
    expect(card).toHaveTextContent("example.com:443");
    expect(card).toHaveTextContent("不发送 HTTP 内容");
    expect(card).toHaveTextContent("不会记住本次授权");
    fireEvent.click(within(card).getByRole("button", { name: "允许本次探测" }));
    await waitFor(() => expect(onResolve).toHaveBeenCalledWith("probe-1", true));

    const observed = { ...proposal, status: "observed" };
    view.rerender(<Conversation
      projectId="project-1" ticket={ticket} events={[event(1, "rescue.network_probe.proposed", { proposal }), event(2, "rescue.network_probe.observed", { proposalId: "probe-1", record: { proposal: observed, evidence: { id: "evidence-r1", summary: "TCP 可达", structured: { connected: true } }, detail: "已建立 TCP 连接", updatedAt: "2026-11-10T00:00:02Z" } })]} effect={null} receipt={null} appliedEffect={null} busy={false}
      onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()} onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()}
      onResolveRescueProbe={onResolve} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()} providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);
    const completed = screen.getByRole("article", { name: "Rescue 联网探测" });
    expect(completed).toHaveTextContent("探测完成");
    expect(completed).toHaveTextContent("evidence-r1");
    expect(within(completed).queryByRole("button", { name: "允许本次探测" })).not.toBeInTheDocument();
  });

  it("shows a compact verified candidate comparison without private workspace data", () => {
    render(<Conversation
      projectId="project-1" ticket={ticket} events={[]} effect={null} receipt={null} appliedEffect={null} busy={false}
      races={[{
        race: { schemaVersion: 1, id: "race-1", ticketId: ticket.id, projectFingerprint: "digest", objective: "修复两处实现", state: "awaiting_review", targets: [{ path: "src/a.ts", operation: "modify" }, { path: "tests/a.test.ts", operation: "modify" }], allowedPaths: ["src/a.ts", "tests/a.test.ts"], acceptance: ["npm test"], verdict: { schemaVersion: 1, kind: "winner", winnerId: "candidate-a", rankedCandidateIds: ["candidate-a"], decisions: [], summary: "passed", decidedAt: "2026-11-10T00:00:01Z" }, createdAt: "2026-11-10T00:00:00Z", updatedAt: "2026-11-10T00:00:01Z" },
        candidates: [{ id: "candidate-a", strategy: "minimal_change", state: "passed", provider: "bundled", model: "qwen", diff: "--- a/src/a.ts\n+++ b/src/a.ts\n+fixed", resultDigest: "result", metrics: { modifiedFiles: 2, unrelatedFiles: 0, deletedFiles: 0, changedLines: 3 }, facts: [{ checkId: "required-tests", outcome: "passed", summary: "3 tests passed", required: true, hardFailure: null, command: "npm test", exitCode: 0, evidenceDigests: ["evidence"] }] }],
      }]}
      onCancelRace={vi.fn()} onSubmit={vi.fn()} onCancel={vi.fn()} onRetry={vi.fn()} onExport={vi.fn()} onBranch={vi.fn()}
      onApprove={vi.fn()} onReject={vi.fn()} onRollback={vi.fn()} onExportReceipt={vi.fn()} hasProject onOpenProject={vi.fn()} error={null}
      sidebarOpen inspectorOpen={false} onToggleSidebar={vi.fn()} onToggleInspector={vi.fn()}
      providerStatus="ready" modelProvider="bundled" providerModel="" modelPerformance={null}
    />);

    const card = screen.getByRole("article", { name: "候选验证" });
    expect(card).toHaveTextContent("本地验证已选出推荐方案");
    expect(card).toHaveTextContent("最小改动");
    expect(card).toHaveTextContent("2 文件");
    expect(card).toHaveTextContent("npm test");
    expect(card).not.toHaveTextContent("workspace");
    fireEvent.click(within(card).getByText("最小改动"));
    expect(card).toHaveTextContent("3 tests passed");
    fireEvent.click(within(card).getByText("查看候选 Diff"));
    expect(card).toHaveTextContent("+fixed");
  });
});
