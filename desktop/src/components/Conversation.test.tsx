import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { chooseContextAttachments } from "../bridge/desktop";
import type { RuntimeEvent, TicketSummary } from "../domain/types";
import { buildTimeline, Conversation } from "./Conversation";

vi.mock("../bridge/desktop", async (importOriginal) => ({
  ...await importOriginal<typeof import("../bridge/desktop")>(),
  chooseContextAttachments: vi.fn(async () => null),
}));

afterEach(cleanup);

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
});
