import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { bootstrap } from "../bridge/desktop";
import * as desktopBridge from "../bridge/desktop";
import { Inspector } from "../components/Inspector";
import { App, detectStartingProviderStatus, effectFrom, eventRequiresBootstrapRefresh, friendlyError, mergeBootstrapSnapshot, mergeRuntimeEvents, modelPerformanceFrom, providerStatusFromRuntime, resolveTheme, runtimeCoreReady, shouldPollProviderStatus } from "./App";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function openInspector() {
  const toggle = screen.queryByRole("button", { name: "展开任务面板" });
  if (toggle) fireEvent.click(toggle);
  return screen.getByRole("tablist", { name: "任务上下文" });
}

function selectInspectorTab(name: "Diff" | "文件" | "终端") {
  fireEvent.click(within(openInspector()).getByRole("tab", { name }));
}

describe("UCode desktop", () => {
  it("removes legacy transport wording from user-facing model failures", () => {
    expect(friendlyError(new Error("legacy agent returned an invalid response: 本地代码模型没有生成有效候选，已安全停止。")))
      .toBe("本地代码模型没有生成有效候选，已安全停止。");
  });

  it("maps live runtime health to an honest provider state", () => {
    const base = { services: [], runtimeRoot: "E:\\UCode", owned: true, starting: false, restartAttempts: 0, modelProfile: "qwen2-5-coder-1-5b-q4-k-m", checkedAt: new Date().toISOString(), lastError: null };
    expect(providerStatusFromRuntime({ ...base, overall: "ready" })).toBe("ready");
    expect(providerStatusFromRuntime({ ...base, overall: "starting" })).toBe("offline");
    expect(providerStatusFromRuntime({ ...base, overall: "degraded" })).toBe("unavailable");
    expect(providerStatusFromRuntime({ ...base, overall: "stopped" })).toBe("unavailable");
    expect(providerStatusFromRuntime({ ...base, overall: "missing", runtimeRoot: null })).toBe("unavailable");
    const progressive = { ...base, overall: "starting" as const, services: [
      { id: "agent", label: "Agent", status: "ready" as const },
      { id: "controller", label: "Controller", status: "starting" as const },
      { id: "coder", label: "Coder", status: "ready" as const },
    ] };
    expect(runtimeCoreReady(progressive)).toBe(true);
    expect(providerStatusFromRuntime(progressive)).toBe("ready");
  });

  it("keeps polling both starting and initially unavailable runtimes until they converge", () => {
    expect(shouldPollProviderStatus("offline", false)).toBe(true);
    expect(shouldPollProviderStatus("unavailable", false)).toBe(true);
    expect(shouldPollProviderStatus("ready", false)).toBe(false);
    expect(shouldPollProviderStatus("unavailable", true)).toBe(false);
  });

  it("confirms a cold-starting runtime with the authoritative Agent bootstrap", async () => {
    const snapshot = await bootstrap();
    const diagnostics = {
      overall: "starting" as const,
      services: [],
      runtimeRoot: snapshot.portableRoot,
      owned: true,
      starting: true,
      restartAttempts: 0,
      modelProfile: "qwen2-5-coder-1-5b-q4-k-m",
      checkedAt: new Date().toISOString(),
      lastError: null,
    };
    expect(await detectStartingProviderStatus(
      async () => diagnostics,
      async () => ({ ...snapshot, providerStatus: "ready" }),
    )).toBe("ready");
    expect(await detectStartingProviderStatus(
      async () => diagnostics,
      async () => ({ ...snapshot, providerStatus: "offline" }),
    )).toBe("offline");
  });

  it("derives model performance only from the active ticket", async () => {
    const snapshot = await bootstrap();
    const firstTicket = snapshot.tickets[0].id;
    const secondTicket = snapshot.tickets[1].id;
    const events = [
      { sequence: 1, ticketId: firstTicket, eventType: "provider.started", payload: {}, createdAt: "2026-11-11T00:00:00.000Z" },
      { sequence: 2, ticketId: firstTicket, eventType: "provider.delta", payload: { delta: "第一", streamSequence: 1 }, createdAt: "2026-11-11T00:00:01.000Z" },
      { sequence: 3, ticketId: firstTicket, eventType: "provider.delta", payload: { delta: "回答", streamSequence: 2 }, createdAt: "2026-11-11T00:00:01.500Z" },
      { sequence: 4, ticketId: firstTicket, eventType: "delivery.ready", payload: { content: "第一回答" }, createdAt: "2026-11-11T00:00:01.600Z" },
      { sequence: 5, ticketId: secondTicket, eventType: "provider.started", payload: {}, createdAt: "2026-11-11T00:00:03.000Z" },
      { sequence: 6, ticketId: secondTicket, eventType: "provider.delta", payload: { delta: "不要混入", streamSequence: 1 }, createdAt: "2026-11-11T00:00:03.200Z" },
    ];
    expect(modelPerformanceFrom({ ...snapshot, events }, firstTicket)).toMatchObject({ outcome: "completed", characterCount: 4 });
    expect(modelPerformanceFrom({ ...snapshot, events }, secondTicket)).toMatchObject({ outcome: "generating", characterCount: 4 });
    expect(modelPerformanceFrom({ ...snapshot, events }, null)).toBeNull();
  });

  it("never revives an approval after transaction recovery becomes required", async () => {
    const snapshot = await bootstrap();
    const ticketId = snapshot.tickets[0].id;
    const events = [
      { sequence: 1, ticketId, eventType: "effect.proposed", payload: { id: "effect-recovery", changes: [{ path: "a.py", operation: "modify", before: "old", after: "new" }] }, createdAt: new Date().toISOString() },
      { sequence: 2, ticketId, eventType: "effect.recovery_required", payload: { effectId: "effect-recovery", message: "automatic recovery could not be verified" }, createdAt: new Date().toISOString() },
    ];
    expect(effectFrom({ ...snapshot, events }, ticketId)).toBeNull();
  });

  it("hides a historical approval after its candidate race becomes stale", async () => {
    const snapshot = await bootstrap();
    const ticketId = snapshot.tickets[0].id;
    const events = [
      { sequence: 1, ticketId, eventType: "effect.proposed", payload: { id: "race-effect", changes: [{ path: "a.py", operation: "modify", before: "old", after: "new" }] }, createdAt: new Date().toISOString() },
      { sequence: 2, ticketId, eventType: "race.decided", payload: { race: { state: "stale" } }, createdAt: new Date().toISOString() },
    ];
    expect(effectFrom({ ...snapshot, events }, ticketId)).toBeNull();
  });

  it("hides a legacy approval when bootstrap safely stops an old race protocol", async () => {
    const snapshot = await bootstrap();
    const ticketId = snapshot.tickets[0].id;
    const events = [
      { sequence: 1, ticketId, eventType: "effect.proposed", payload: { id: "legacy-race-effect", path: "a.py", after: "new" }, createdAt: new Date().toISOString() },
      { sequence: 2, ticketId, eventType: "race.legacy_stopped", payload: { raceId: "race-old" }, createdAt: new Date().toISOString() },
    ];
    expect(effectFrom({ ...snapshot, events }, ticketId)).toBeNull();
  });

  it("resolves the system theme and suppresses live deltas when streaming is disabled", async () => {
    expect(resolveTheme("system", true)).toBe("light");
    expect(resolveTheme("system", false)).toBe("dark");
    expect(resolveTheme("light", false)).toBe("light");

    const snapshot = await bootstrap();
    const current = { ...snapshot, settings: { ...snapshot.settings, streamEnabled: false }, events: [], lastSequence: 10 };
    const next = mergeRuntimeEvents(current, [
      { sequence: 11, ticketId: "ticket-demo", eventType: "provider.delta", payload: { delta: "不可逐字显示" }, createdAt: new Date().toISOString() },
      { sequence: 12, ticketId: "ticket-demo", eventType: "delivery.ready", payload: { content: "最终回答" }, createdAt: new Date().toISOString() },
    ]);
    expect(next.lastSequence).toBe(12);
    expect(next.events).toHaveLength(1);
    expect(next.events[0].eventType).toBe("delivery.ready");
  });

  it("refreshes durable ticket state after a race reaches its verdict", async () => {
    const snapshot = await bootstrap();
    const current = { ...snapshot, tickets: snapshot.tickets.map((ticket) => ({ ...ticket, state: "running" as const })), lastSequence: 10 };
    const fresh = { ...snapshot, tickets: snapshot.tickets.map((ticket) => ({ ...ticket, state: "completed" as const })), lastSequence: 11 };
    expect(eventRequiresBootstrapRefresh({ sequence: 11, ticketId: "ticket-demo", eventType: "race.decided", payload: {}, createdAt: new Date().toISOString() })).toBe(true);
    expect(eventRequiresBootstrapRefresh({ sequence: 10, ticketId: "ticket-demo", eventType: "race.validating", payload: {}, createdAt: new Date().toISOString() })).toBe(false);
    expect(mergeBootstrapSnapshot(fresh, current).tickets[0].state).toBe("completed");
  });

  it("renders the portable conversation workspace", async () => {
    render(<App />);
    expect(await screen.findByLabelText("UCode Portable")).toBeInTheDocument();
    expect(document.querySelector(".sidebar .projects")).toBeInTheDocument();
    expect(document.querySelector(".sidebar .conversations")).toBeInTheDocument();
    expect(document.querySelector(".portable-state")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "修复加法函数" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "批准选中的 1 个文件" })).toBeInTheDocument();
    expect(screen.getByText(/我检查了加法函数/)).toBeInTheDocument();
    openInspector();
    expect(screen.getByRole("region", { name: "calculator.py 差异审查" })).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "逐行差异" })).toHaveTextContent("return a + b");
    expect(screen.getByLabelText("任务清单")).toHaveTextContent("等待批准文件修改");
    expect(screen.getByText((_, node) => node?.textContent === "本轮 2/3")).toBeInTheDocument();
  });

  it("opens the portable onboarding checklist from Help without covering the conversation", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "修复加法函数" });
    fireEvent.click(screen.getByRole("button", { name: "? 帮助" }));
    const tabs = screen.getByRole("tablist", { name: "任务上下文" });
    expect(within(tabs).getByRole("tab", { name: "开始" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("region", { name: "UCode 入门已完成" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "修复加法函数" })).toBeInTheDocument();
  });

  it("opens real inspector views and model status", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "修复加法函数" });
    selectInspectorTab("文件");
    expect(await screen.findByRole("tree", { name: "项目文件" })).toHaveTextContent("calculator.py");
    const filter = screen.getByLabelText("筛选项目文件");
    fireEvent.change(filter, { target: { value: "test_" } });
    expect(screen.getByRole("tree", { name: "项目文件" })).toHaveTextContent("test_calculator.py");
    expect(screen.getByRole("tree", { name: "项目文件" })).not.toHaveTextContent(/^calculator.py$/);
    fireEvent.change(filter, { target: { value: "calculator.py" } });
    fireEvent.click(document.querySelector<HTMLElement>('button[role="treeitem"] b[title="calculator.py"]')!.closest("button")!);
    expect(await screen.findByRole("region", { name: "calculator.py 文件内容" })).toHaveTextContent("return a - b");
    expect(within(openInspector()).queryByRole("tab", { name: "Git" })).not.toBeInTheDocument();
    expect(within(openInspector()).queryByRole("tab", { name: "Agents" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /内置模型/ }));
    expect(screen.getByRole("dialog", { name: "模型状态" })).toHaveTextContent("UCode 安全路由器");
    expect(screen.getByRole("dialog", { name: "模型状态" })).toHaveTextContent("已验证的本地模型包");
  });

  it("brings a newly proposed effect into the Diff review tab", async () => {
    const ticket = { id: "ticket-review", projectId: "project-review", title: "审查修改", request: "修改文件", state: "running" } as never;
    const project = { id: "project-review", name: "review-project", path: "E:\\review-project", trusted: true } as never;
    const props = { ticket, project, events: [], onExpandedChange: () => undefined, onOpenProject: async () => undefined, onClose: () => undefined };
    const view = render(<Inspector {...props} effect={null} />);
    fireEvent.click(screen.getByRole("tab", { name: "文件" }));
    expect(screen.getByRole("tab", { name: "文件" })).toHaveAttribute("aria-selected", "true");

    view.rerender(<Inspector {...props} effect={{ id: "effect-review", changes: [
      { path: "knapsack.py", operation: "modify", before: "value = 1\n", after: "# reviewed\nvalue = 1\n", beforeDigest: null, afterDigest: null },
      { path: "README.md", operation: "create", before: "", after: "# Review\n", beforeDigest: null, afterDigest: null },
    ], risk: "write" }} />);

    await waitFor(() => expect(screen.getByRole("tab", { name: "Diff" })).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("region", { name: "knapsack.py 差异审查" })).toHaveTextContent("# reviewed");
    fireEvent.click(screen.getByRole("tab", { name: /README\.md/ }));
    expect(screen.getByRole("region", { name: "README.md 差异审查" })).toHaveTextContent("# Review");
  });

  it("refreshes the Files view after an applied project mutation", async () => {
    const initial = [
      { path: "calculator.py", name: "calculator.py", isDirectory: false, depth: 0, size: 38 },
      { path: "test_calculator.py", name: "test_calculator.py", isDirectory: false, depth: 0, size: 74 },
    ];
    const created = [
      ...initial,
      { path: "fibonacci.py", name: "fibonacci.py", isDirectory: false, depth: 0, size: 96 },
      { path: "test_fibonacci.py", name: "test_fibonacci.py", isDirectory: false, depth: 0, size: 112 },
    ];
    const listFiles = vi.spyOn(desktopBridge, "listProjectFiles")
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(created);
    const ticket = { id: "ticket-files", projectId: "project-files", title: "创建文件", request: "创建斐波那契模块", state: "running" } as never;
    const project = { id: "project-files", name: "files-project", path: "E:\\files-project", trusted: true } as never;
    const props = { ticket, project, effect: null, onExpandedChange: () => undefined, onOpenProject: async () => undefined, onClose: () => undefined };
    const view = render(<Inspector {...props} events={[]} />);

    fireEvent.click(screen.getByRole("tab", { name: "文件" }));
    expect(await screen.findByRole("tree", { name: "项目文件" })).toHaveTextContent("calculator.py");
    expect(screen.getByRole("tree", { name: "项目文件" })).not.toHaveTextContent("fibonacci.py");

    view.rerender(<Inspector {...props} events={[{
      sequence: 12,
      ticketId: "ticket-files",
      eventType: "effect.applied",
      payload: { effectId: "effect-files" },
      createdAt: "2026-11-17T10:00:00.000Z",
    }]} />);

    await waitFor(() => expect(screen.getByRole("tree", { name: "项目文件" })).toHaveTextContent("fibonacci.py"));
    expect(screen.getByRole("tree", { name: "项目文件" })).toHaveTextContent("test_fibonacci.py");
    expect(listFiles).toHaveBeenCalledTimes(2);
  });

  it("uses structured one-turn attachments without putting disk paths in the message", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "修复加法函数" });
    fireEvent.click(screen.getByRole("button", { name: "添加上下文文件" }));
    const context = await screen.findByLabelText("已添加上下文");
    expect(context).toHaveTextContent("calculator.py");
    expect(context).toHaveTextContent("项目");
    expect(context).toHaveTextContent("38 B");
    expect(screen.queryByText(/E:\\UCode\\workspaces/)).not.toBeInTheDocument();

    const input = screen.getByLabelText("任务输入");
    fireEvent.change(input, { target: { value: "解释这个文件" } });
    fireEvent.submit(input.closest("form")!);
    await waitFor(() => expect(screen.queryByLabelText("已添加上下文")).not.toBeInTheDocument());
    expect(screen.getByText("解释这个文件")).toBeInTheDocument();
    expect(screen.queryByText(/本轮本地上下文/)).not.toBeInTheDocument();
  });

  it("automatically uses isolated candidates only for a new verifiable multi-file task", async () => {
    const startRace = vi.spyOn(desktopBridge, "startCandidateRace");
    const normalRun = vi.spyOn(desktopBridge, "runTicket");
    render(<App />);
    await screen.findByRole("heading", { name: "修复加法函数" });
    fireEvent.click(screen.getByRole("button", { name: "＋ 新任务" }));
    const input = screen.getByLabelText("任务输入");
    fireEvent.change(input, { target: { value: "修改 `src/cart.ts` 和 `tests/cart.test.ts`，然后运行 npm test" } });
    fireEvent.submit(input.closest("form")!);

    expect(await screen.findByRole("article", { name: "候选验证" })).toHaveTextContent("正在比较两个隔离方案");
    expect(startRace).toHaveBeenCalledWith(
      expect.any(String),
      [
        { path: "src/cart.ts", operation: "modify" },
        { path: "tests/cart.test.ts", operation: "modify" },
      ],
      ["npm test"],
    );
    expect(normalRun).not.toHaveBeenCalled();
  });

  it("renames a conversation through its action menu", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "修复加法函数" });
    fireEvent.click(screen.getByRole("button", { name: "修复加法函数 会话操作" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "重命名" }));
    const input = screen.getByLabelText("会话名称");
    fireEvent.change(input, { target: { value: "新的会话名称" } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(screen.getByRole("heading", { name: "新的会话名称" })).toBeInTheDocument());
  });

  it("searches conversations and opens persistent settings", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "修复加法函数" });
    const search = screen.getByLabelText("搜索会话");
    fireEvent.change(search, { target: { value: "不存在" } });
    expect(screen.getByText("没有匹配的会话")).toBeInTheDocument();
    fireEvent.change(search, { target: { value: "修复" } });
    expect(screen.getByRole("button", { name: /^修复加法函数\s*等待处理$/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /设置/ }));
    const settings = screen.getByRole("dialog", { name: "UCode 设置" });
    expect(settings).toHaveTextContent("逐 Token 流式输出");
    fireEvent.click(within(settings).getByRole("button", { name: /终端/ }));
    expect(within(settings).getByLabelText("Shell")).toHaveValue("powershell.exe");
    fireEvent.click(within(settings).getByRole("button", { name: /技能/ }));
    expect(await within(settings).findByText(/尚未安装技能/)).toBeInTheDocument();
    expect(within(settings).getByRole("button", { name: "导入目录" })).toBeInTheDocument();
  });

  it("configures default-on portable audio feedback", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "修复加法函数" });
    fireEvent.click(screen.getByRole("button", { name: /设置/ }));
    let settings = screen.getByRole("dialog", { name: "UCode 设置" });
    fireEvent.click(within(settings).getByRole("button", { name: /外观/ }));
    const enabled = within(settings).getByRole("checkbox", { name: /提示音/ });
    expect(within(settings).getByRole("checkbox", { name: /后台任务完成通知/ })).toBeChecked();
    const volume = within(settings).getByLabelText("提示音音量");
    expect(enabled).toBeChecked();
    expect(volume).toHaveValue("55");
    fireEvent.change(volume, { target: { value: "23" } });
    fireEvent.click(within(settings).getByRole("button", { name: "试听提示音" }));
    fireEvent.click(within(settings).getByRole("button", { name: "保存设置" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "UCode 设置" })).not.toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: /设置/ }));
    settings = screen.getByRole("dialog", { name: "UCode 设置" });
    fireEvent.click(within(settings).getByRole("button", { name: /外观/ }));
    expect(within(settings).getByLabelText("提示音音量")).toHaveValue("23");
    fireEvent.click(within(settings).getByRole("checkbox", { name: /提示音/ }));
    expect(within(settings).getByLabelText("提示音音量")).toBeDisabled();
    expect(within(settings).getByRole("button", { name: "试听提示音" })).toBeDisabled();
  });

  it("clears only the local notification identity after confirmation", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<App />);
    await screen.findByRole("heading", { name: "修复加法函数" });
    fireEvent.click(screen.getByRole("button", { name: /设置/ }));
    const settings = screen.getByRole("dialog", { name: "UCode 设置" });
    fireEvent.click(within(settings).getByRole("button", { name: /外观/ }));
    const clear = await within(settings).findByRole("button", { name: "清除本机通知身份" });
    await waitFor(() => expect(clear).not.toBeDisabled());
    fireEvent.click(clear);
    expect(confirm).toHaveBeenCalledWith(expect.stringContaining("不会删除对话或项目"));
    expect(await within(settings).findByText("已清除本机通知身份，并关闭后台完成通知。")).toBeInTheDocument();
    expect(within(settings).getByRole("checkbox", { name: /后台任务完成通知/ })).not.toBeChecked();
    expect(within(settings).getByText("未注册")).toBeInTheDocument();
    confirm.mockRestore();
  });

  it("resizes persistent panes with the keyboard and exposes accessible splitters", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "修复加法函数" });
    const left = screen.getByRole("separator", { name: "调整会话侧栏宽度" });
    expect(left).toHaveAttribute("aria-valuenow", "276");
    fireEvent.keyDown(left, { key: "ArrowRight" });
    await waitFor(() => expect(screen.getByRole("separator", { name: "调整会话侧栏宽度" })).toHaveAttribute("aria-valuenow", "284"));

    const openPanel = screen.queryByRole("button", { name: "展开任务面板" });
    if (openPanel) fireEvent.click(openPanel);
    const right = screen.getByRole("separator", { name: "调整任务面板宽度" });
    expect(right).toHaveAttribute("aria-valuenow", "404");
    fireEvent.keyDown(right, { key: "ArrowLeft" });
    await waitFor(() => expect(screen.getByRole("separator", { name: "调整任务面板宽度" })).toHaveAttribute("aria-valuenow", "412"));
  });

  it("switches real project context and filters conversations by project", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "修复加法函数" });
    const calculator = screen.getByRole("button", { name: /calculator-app/ });
    const notes = screen.getByRole("button", { name: /notes-app/ });
    expect(calculator).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(notes);
    await screen.findByRole("heading", { name: "整理发布说明" });
    expect(notes).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("button", { name: /^修复加法函数\s*等待处理$/ })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^整理发布说明\s*已完成$/ })).toBeInTheDocument();

    fireEvent.click(calculator);
    await screen.findByRole("heading", { name: "修复加法函数" });
    expect(calculator).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("button", { name: /^整理发布说明\s*已完成$/ })).not.toBeInTheDocument();
  });

  it("progressively configures and tests an OpenAI-compatible provider", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "修复加法函数" });
    fireEvent.click(screen.getByRole("button", { name: /设置/ }));
    const settings = screen.getByRole("dialog", { name: "UCode 设置" });
    expect(within(settings).queryByLabelText("Endpoint")).not.toBeInTheDocument();
    fireEvent.click(within(settings).getByRole("button", { name: /OpenAI 兼容端点/ }));
    expect(within(settings).getByLabelText("Endpoint")).toHaveValue("http://127.0.0.1:1234/v1");
    fireEvent.change(within(settings).getByLabelText("Model"), { target: { value: "qwen-local" } });
    fireEvent.click(within(settings).getByRole("button", { name: "测试连接" }));
    expect(await within(settings).findByText("连接成功 · qwen-local")).toBeInTheDocument();
    expect(settings).toHaveTextContent("密钥只保存在当前 Windows 用户凭据中");
  });

  it("keeps the approved terminal session output when switching inspector tabs", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "修复加法函数" });
    selectInspectorTab("终端");
    const input = await screen.findByLabelText("终端命令");
    await waitFor(() => expect(input).not.toBeDisabled());
    fireEvent.change(input, { target: { value: "Get-ChildItem" } });
    fireEvent.submit(input.closest("form")!);
    const approval = screen.getByText("批准写入交互终端？").closest<HTMLElement>(".inline-approval")!;
    fireEvent.click(within(approval).getByRole("button", { name: "批准执行" }));
    await screen.findByText(/demo output/);
    selectInspectorTab("文件");
    await screen.findByRole("tree", { name: "项目文件" });
    selectInspectorTab("终端");
    expect(screen.getByText(/demo output/)).toBeInTheDocument();
  });

  it("keeps Git and Agent management out of the primary workflow", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "修复加法函数" });
    const tabs = openInspector();
    expect(within(tabs).getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["Diff", "文件", "终端"]);
  });
});
