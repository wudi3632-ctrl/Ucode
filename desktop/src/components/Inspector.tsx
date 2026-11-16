import { useEffect, useMemo, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { applyGitOperation, cancelSubagents, closeTerminalSession, createWorktree, interruptTerminalSession, listProjectFiles, listSubagents, listWorktrees, mergeSubagentChange, readGitDetails, readProjectFile, readTerminalSession, removeWorktree, resizeTerminalSession, resolveSubagentEffect, startSubagents, startTerminalSession, subscribeTerminal, writeTerminalCommand } from "../bridge/desktop";
import type { AssignmentSummary, FilePreview, GitDetails, OnboardingState, OnboardingStepId, ProjectFileEntry, ProjectSummary, ProposedEffect, RuntimeEvent, SubagentRequest, TerminalState, TicketSummary, WorktreeEntry } from "../domain/types";
import { codeFontSize as normalizedCodeFontSize } from "../domain/typography";
import { buildDiffRows } from "../domain/diff";
import { completedOnboardingState, OnboardingChecklist, OnboardingTip } from "./Onboarding";

interface InspectorProps {
  ticket: TicketSummary | null;
  project: ProjectSummary | null;
  effect: ProposedEffect | null;
  events: RuntimeEvent[];
  codeFontSize?: number;
  onExpandedChange: (expanded: boolean) => void;
  onOpenProject: (path: string) => Promise<void>;
  onClose: () => void;
  onboarding?: OnboardingState;
  openOnboarding?: boolean;
  onOnboardingStep?: (step: OnboardingStepId) => Promise<void>;
  onSkipOnboardingStep?: (step: OnboardingStepId) => Promise<void>;
  onResetOnboarding?: () => Promise<void>;
  onDismissOnboardingTip?: (tipId: string) => Promise<void>;
  onChooseProject?: () => Promise<void>;
  onCreateDemo?: () => Promise<void>;
}

type InspectorTab = "guide" | "diff" | "files" | "terminal" | "git" | "agents";

const tabOrder: InspectorTab[] = ["guide", "diff", "files", "terminal", "git", "agents"];
const tabLabels: Record<InspectorTab, string> = { guide: "开始", diff: "Diff", files: "文件", terminal: "终端", git: "Git", agents: "Agents" };

export function Inspector({ ticket, project, effect, events, codeFontSize = 14, onExpandedChange, onOpenProject, onClose, onboarding = completedOnboardingState, openOnboarding = false, onOnboardingStep = async () => undefined, onSkipOnboardingStep = async () => undefined, onResetOnboarding = async () => undefined, onDismissOnboardingTip = async () => undefined, onChooseProject = async () => undefined, onCreateDemo = async () => undefined }: InspectorProps) {
  const [tab, setTab] = useState<InspectorTab>("diff");
  const [visitedTabs, setVisitedTabs] = useState<Set<InspectorTab>>(() => new Set(["diff"]));
  const [direction, setDirection] = useState(1);
  const [files, setFiles] = useState<ProjectFileEntry[]>([]);
  const [preview, setPreview] = useState<FilePreview | null>(null);
  const [git, setGit] = useState<GitDetails | null>(null);
  const [loading, setLoading] = useState(false);
  const [panelError, setPanelError] = useState<string | null>(null);
  const ticketEvents = useMemo(() => events.filter((event) => event.ticketId === ticket?.id), [events, ticket?.id]);
  const steps = useMemo(() => currentTurnSteps(ticket, ticketEvents), [ticket, ticketEvents]);
  const showGuide = openOnboarding || onboarding.welcomeState !== "completed";
  const availableTabs = showGuide ? tabOrder : tabOrder.filter((item) => item !== "guide");

  useEffect(() => {
    if (!openOnboarding) return;
    setDirection(-1);
    setTab("guide");
    onExpandedChange(false);
  }, [openOnboarding]);

  useEffect(() => {
    if (showGuide || tab !== "guide") return;
    setDirection(1);
    setTab("diff");
  }, [showGuide, tab]);

  useEffect(() => {
    setPreview(null);
    setGit(null);
    onExpandedChange(false);
  }, [project?.id]);

  useEffect(() => {
    if (!effect) return;
    setDirection(-1);
    setTab("diff");
    setVisitedTabs((current) => new Set(current).add("diff"));
    setPanelError(null);
    onExpandedChange(false);
  }, [effect?.id]);

  useEffect(() => {
    if (tab !== "files" || !project || files.length) return;
    setLoading(true); setPanelError(null);
    listProjectFiles(project.id).then(setFiles).catch((error) => setPanelError(String(error))).finally(() => setLoading(false));
  }, [tab, project, files.length]);

  useEffect(() => {
    if (tab !== "git" || !project || git) return;
    setLoading(true); setPanelError(null);
    readGitDetails(project.id).then((value) => { setGit(value); if (value.isRepository) void onOnboardingStep("git"); }).catch((error) => setPanelError(String(error))).finally(() => setLoading(false));
  }, [tab, project, git]);

  function selectTab(next: InspectorTab) {
    setDirection(tabOrder.indexOf(next) >= tabOrder.indexOf(tab) ? 1 : -1);
    setTab(next);
    setVisitedTabs((current) => new Set(current).add(next));
    setPanelError(null);
    onExpandedChange(next === "files" && Boolean(preview));
  }

  function onboardingAction(step: OnboardingStepId) {
    if (step === "project") { void onChooseProject(); return; }
    if (step === "conversation") { onClose(); window.setTimeout(() => document.querySelector<HTMLTextAreaElement>('[aria-label="任务输入"]')?.focus(), 0); return; }
    if (step === "review_change") { selectTab("diff"); return; }
    if (step === "terminal") { selectTab("terminal"); return; }
    if (step === "git") { selectTab("git"); return; }
    if (step === "agent") { selectTab("agents"); return; }
    void onOnboardingStep("evidence");
    onClose();
    window.setTimeout(() => document.querySelector<HTMLElement>('[aria-label="结果证据"]')?.scrollIntoView({ behavior: "smooth", block: "center" }), 0);
  }

  async function openFile(entry: ProjectFileEntry) {
    if (!project || entry.isDirectory) return;
    setLoading(true); setPanelError(null);
    try { setPreview(await readProjectFile(project.id, entry.path)); onExpandedChange(true); }
    catch (error) { setPanelError(String(error)); }
    finally { setLoading(false); }
  }

  return (
    <aside className="inspector">
      <nav className="inspector-tabs" aria-label="任务上下文" role="tablist">
        <div>{availableTabs.map((item) => <button key={item} role="tab" className={tab === item ? "selected" : ""} aria-selected={tab === item} onClick={() => selectTab(item)}>{tabLabels[item]}</button>)}</div>
        <button className="inspector-close" aria-label="关闭任务上下文" title="关闭面板" onClick={onClose}>×</button>
      </nav>
      {tab === "guide" ? <div className="inspector-layout onboarding-inspector-layout"><div className="inspector-body"><div className="inspector-view slide-back"><OnboardingChecklist state={onboarding} onAction={onboardingAction} onSkip={onSkipOnboardingStep} onReset={onResetOnboarding} onCreateDemo={onCreateDemo} /></div></div></div> : ticket ? (
        <div className="inspector-layout">
          <div className="inspector-body">
            {tab !== "terminal" && <div className={`inspector-view slide-${direction > 0 ? "forward" : "back"}`} key={`${tab}-${project?.id ?? "none"}`}>
              {tab === "diff" && <><OnboardingTip id="diff" title="先审查，再写入" dismissed={onboarding.dismissedTips.includes("diff")} onDismiss={onDismissOnboardingTip}>Diff 只展示候选修改。只有你在对话中批准选中的文件，UCode 才会重新校验并写入。</OnboardingTip><DiffView effect={effect} /></>}
              {tab === "files" && <FilesView files={files} preview={preview} loading={loading} onOpen={openFile} onBack={() => { setPreview(null); onExpandedChange(false); }} />}
              {tab === "git" && <><OnboardingTip id="git" title="Git 操作仍由你控制" dismissed={onboarding.dismissedTips.includes("git")} onDismiss={onDismissOnboardingTip}>查看不会改变仓库；暂存、提交和分支操作会再次要求批准。</OnboardingTip><GitWorkspace project={project} snapshot={git} loading={loading} onChange={setGit} onOpenProject={onOpenProject} /></>}
              {tab === "agents" && <><OnboardingTip id="agents" title="编辑 Agent 使用隔离工作区" dismissed={onboarding.dismissedTips.includes("agents")} onDismiss={onDismissOnboardingTip}>只读任务不会写文件；编辑任务先在独立 Worktree 生成 Diff，再由你分别批准写入和合并。</OnboardingTip><SubagentWorkspace ticket={ticket} events={ticketEvents} onCompleted={() => void onOnboardingStep("agent")} /></>}
              {panelError && <div className="panel-error">{panelError}</div>}
            </div>}
            {visitedTabs.has("terminal") && <div className={`terminal-persistent ${tab === "terminal" ? "visible slide-forward" : "hidden"}`}><OnboardingTip id="terminal" title="每条命令先审批" dismissed={onboarding.dismissedTips.includes("terminal")} onDismiss={onDismissOnboardingTip}>终端会话持续存在；命令写入前会展示内容和工作目录，运行中可发送 Ctrl+C。</OnboardingTip><TerminalWorkspace project={project} events={ticketEvents} codeFontSize={codeFontSize} onCommandAccepted={() => void onOnboardingStep("terminal")} /></div>}
          </div>
          <InspectorStatus steps={steps} projectName={project?.name ?? "未选择项目"} />
        </div>
      ) : <div className="inspector-empty"><b>还没有活动任务</b><p>开始对话后，文件、执行记录和 Git 状态会在这里展开。</p></div>}
    </aside>
  );
}

function InspectorStatus({ steps, projectName }: { steps: TaskStep[]; projectName: string }) {
  const done = steps.filter((step) => step.state === "done").length;
  return <details className="inspector-status"><summary><span className="status-dot" /><b>本轮 {done}/{steps.length}</b><small>{projectName}</small><i>⌃</i></summary><div><TaskList steps={steps} /></div></details>;
}

function DiffView({ effect }: { effect: ProposedEffect | null }) {
  const [activePath, setActivePath] = useState("");
  if (!effect) return <div className="context-placeholder"><span>⌘</span><p>当前轮次没有待审批的文件修改。</p></div>;
  const change = effect.changes.find((item) => item.path === activePath) ?? effect.changes[0];
  if (!change) return <div className="context-placeholder"><span>⌘</span><p>修改批次没有可审查的文件。</p></div>;
  const rows = buildDiffRows(change.before, change.after);
  const additions = rows.filter((row) => row.kind === "addition").length;
  const deletions = rows.filter((row) => row.kind === "deletion").length;
  return <section className="review-surface" aria-label={`${change.path} 差异审查`}>
    {effect.changes.length > 1 && <div className="diff-file-tabs" role="tablist" aria-label="批次文件">
      {effect.changes.map((item) => <button role="tab" aria-selected={item.path === change.path} className={item.path === change.path ? "selected" : ""} key={item.path} onClick={() => setActivePath(item.path)}><span>{item.path.split(/[\\/]/).at(-1)}</span><small>{item.operation === "create" ? "A" : item.operation === "delete" ? "D" : "M"}</small></button>)}
    </div>}
    <div className="review-header">
      <div><b title={change.path}>{change.path.split(/[\\/]/).at(-1)}</b><small title={change.path}>{change.path}</small></div>
      <div className="review-summary"><span className="deletion">−{deletions}</span><span className="addition">+{additions}</span><i>待审批</i></div>
    </div>
    <div className="diff-reader" role="table" aria-label="逐行差异">
      {rows.map((row, index) => <div className={`diff-line ${row.kind}`} role="row" key={`${row.kind}-${row.oldLine}-${row.newLine}-${index}`}>
        <span role="cell">{row.oldLine || ""}</span><span role="cell">{row.newLine || ""}</span><i aria-hidden="true">{row.kind === "addition" ? "+" : row.kind === "deletion" ? "−" : row.kind === "omission" ? "⋯" : ""}</i><code role="cell">{row.content || " "}</code>
      </div>)}
    </div>
  </section>;
}

function FilesView({ files, preview, loading, onOpen, onBack }: { files: ProjectFileEntry[]; preview: FilePreview | null; loading: boolean; onOpen: (entry: ProjectFileEntry) => void; onBack: () => void }) {
  const [wrap, setWrap] = useState(true);
  const [copied, setCopied] = useState(false);
  const [selectedLine, setSelectedLine] = useState<number | null>(null);
  const [query, setQuery] = useState("");
  if (loading && !files.length && !preview) return <PanelLoading label="正在建立文件索引" />;
  if (preview) {
    const entry = files.find((item) => item.path === preview.path);
    const lines = preview.content.split(/\r?\n/);
    const copy = async () => {
      await navigator.clipboard.writeText(preview.content);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1200);
    };
    return <div className="file-preview">
      <div className="review-header file-reader-header">
        <button className="file-reader-back" onClick={onBack} aria-label="返回文件列表" title="返回文件列表">←</button>
        <div><b title={preview.path}>{preview.path.split(/[\\/]/).at(-1)}</b><small title={preview.path}>{preview.path}{entry ? ` · ${formatBytes(entry.size)}` : ""} · {lines.length} 行</small></div>
        <div className="file-reader-actions">
          <button onClick={() => setWrap((value) => !value)} aria-label={wrap ? "关闭自动换行" : "开启自动换行"} aria-pressed={wrap} title={wrap ? "关闭自动换行" : "开启自动换行"}>{wrap ? "换行" : "不换行"}</button>
          <button onClick={() => void copy()} aria-label="复制文件内容" title="复制文件内容">{copied ? "已复制" : "复制"}</button>
        </div>
      </div>
      <div className={`file-code-scroller ${wrap ? "wrap" : "nowrap"}`} role="region" aria-label={`${preview.path} 文件内容`}>
        <div className="file-code-lines">
          {lines.map((line, index) => <button type="button" className={`code-line ${selectedLine === index + 1 ? "selected" : ""}`} key={index} onClick={() => setSelectedLine(index + 1)} aria-label={`第 ${index + 1} 行`}><span>{index + 1}</span><code>{line || " "}</code></button>)}
        </div>
      </div>
      {preview.truncated && <small>文件较大，仅显示前 200,000 个字符。</small>}
    </div>;
  }
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const visibleFiles = normalizedQuery ? files.filter((entry) => !entry.isDirectory && entry.path.toLocaleLowerCase().includes(normalizedQuery)) : files;
  return <div className="file-browser">
    <div className="file-browser-heading"><div><b>项目文件</b><small>{files.filter((entry) => !entry.isDirectory).length} 个文件</small></div><label><span>⌕</span><input aria-label="筛选项目文件" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="按名称筛选" /></label></div>
    <div className="file-tree" role="tree" aria-label="项目文件">{visibleFiles.length ? visibleFiles.map((entry) => <button role="treeitem" aria-expanded={entry.isDirectory ? true : undefined} key={entry.path} style={{ paddingLeft: 8 + (normalizedQuery ? 0 : entry.depth * 14) }} className={entry.isDirectory ? "directory" : "file"} onClick={() => onOpen(entry)} disabled={entry.isDirectory}><span>{entry.isDirectory ? "▾" : fileGlyph(entry.name)}</span><b title={entry.path}>{normalizedQuery ? entry.path : entry.name}</b>{!entry.isDirectory && <small>{formatBytes(entry.size)}</small>}</button>) : <div className="file-filter-empty">没有匹配的文件</div>}</div>
  </div>;
}

function fileGlyph(name: string) {
  const extension = name.split(".").at(-1)?.toLocaleLowerCase();
  if (["ts", "tsx", "js", "jsx", "rs", "py", "go", "java", "c", "cpp"].includes(extension ?? "")) return "‹›";
  if (["json", "toml", "yaml", "yml"].includes(extension ?? "")) return "{}";
  if (["md", "txt"].includes(extension ?? "")) return "≡";
  return "·";
}

function TerminalWorkspace({ project, events, codeFontSize, onCommandAccepted }: { project: ProjectSummary | null; events: RuntimeEvent[]; codeFontSize: number; onCommandAccepted: () => void }) {
  const [command, setCommand] = useState("");
  const [proposal, setProposal] = useState<string | null>(null);
  const [mode, setMode] = useState<"shell" | "log">("shell");
  const [error, setError] = useState<string | null>(null);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [output, setOutput] = useState("");
  const [status, setStatus] = useState<TerminalState>("starting");
  const [restartNonce, setRestartNonce] = useState(0);
  const screenRef = useRef<HTMLDivElement>(null);
  const terminalHostRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const lastSequenceRef = useRef(-1);
  const stickToBottomRef = useRef(true);
  const visible = events.flatMap((event) => {
    const label = eventLabel(event.eventType);
    return label ? [{ event, label }] : [];
  });
  const running = status === "running" || status === "interrupting";

  function appendChunks(chunks: Array<{ sequence: number; data: string }>, truncated = false) {
    const ordered = [...chunks].sort((left, right) => left.sequence - right.sequence);
    let appended = "";
    for (const chunk of ordered) {
      if (chunk.sequence <= lastSequenceRef.current) continue;
      if (chunk.sequence !== lastSequenceRef.current + 1) break;
      lastSequenceRef.current = chunk.sequence;
      appended += chunk.data;
    }
    if (!appended && !truncated) return;
    if (truncated) terminalRef.current?.writeln("\r\n\x1b[2m— 较早的终端输出已清理 —\x1b[0m");
    if (appended) terminalRef.current?.write(appended);
    setOutput((current) => `${current}${truncated && !current.includes("较早的终端输出已清理") ? "\r\n— 较早的终端输出已清理 —\r\n" : ""}${appended}`.slice(-520_000));
  }

  useEffect(() => {
    if (!terminalHostRef.current) return;
    // jsdom has no canvas/layout engine; the transcript below remains the
    // deterministic test surface while packaged WebView2 uses xterm.
    if (navigator.userAgent.toLowerCase().includes("jsdom")) return;
    const fit = new FitAddon();
    const terminal = new Terminal({
      allowTransparency: true,
      convertEol: false,
      cursorBlink: false,
      disableStdin: true,
      fontFamily: "Cascadia Mono, Consolas, monospace",
      fontSize: normalizedCodeFontSize(codeFontSize),
      lineHeight: 1.35,
      scrollback: 5000,
      theme: { background: "#0d0f0e", foreground: "#d5d8d6", cursor: "#8ea89a", selectionBackground: "#33473d" },
    });
    terminal.loadAddon(fit);
    terminal.open(terminalHostRef.current);
    terminalRef.current = terminal;
    fitAddonRef.current = fit;
    requestAnimationFrame(() => { try { fit.fit(); } catch { /* hidden during tab transition */ } });
    return () => { terminalRef.current = null; fitAddonRef.current = null; terminal.dispose(); };
  }, []);

  useEffect(() => {
    if (!terminalRef.current) return;
    terminalRef.current.options.fontSize = normalizedCodeFontSize(codeFontSize);
    requestAnimationFrame(() => { try { fitAddonRef.current?.fit(); } catch { /* hidden panel */ } });
  }, [codeFontSize]);

  useEffect(() => {
    if (!project) return;
    let disposed = false;
    let activeSession: string | null = null;
    let unlisten: () => void = () => {};
    terminalRef.current?.reset();
    setStatus("starting"); setOutput(""); setError(null); lastSequenceRef.current = -1;
    void subscribeTerminal(
      (event) => { if (event.sessionId === activeSession) appendChunks([event]); },
      (event) => { if (event.sessionId === activeSession) setStatus(event.status); },
    ).then((stop) => {
      unlisten = stop;
      return startTerminalSession(project.id);
    }).then((session) => {
      if (disposed) { void closeTerminalSession(session.sessionId); return; }
      activeSession = session.sessionId; setSessionId(session.sessionId); setStatus(session.state);
      return readTerminalSession(session.sessionId, null, 256);
    }).then((snapshot) => {
      if (!snapshot || disposed) return;
      appendChunks(snapshot.chunks, snapshot.truncated); setStatus(snapshot.state);
    }).catch((cause) => { if (!disposed) { setError(String(cause)); setStatus("exited"); } });
    return () => { disposed = true; unlisten(); if (activeSession) void closeTerminalSession(activeSession); };
  }, [project?.id, restartNonce]);

  useEffect(() => {
    if (!sessionId || ["closed", "exited", "failed"].includes(status)) return;
    let disposed = false;
    const poll = async () => {
      try {
        const snapshot = await readTerminalSession(sessionId, lastSequenceRef.current >= 0 ? lastSequenceRef.current : null, 256);
        if (!disposed) { appendChunks(snapshot.chunks, snapshot.truncated); setStatus(snapshot.state); }
      } catch (cause) {
        if (!disposed) setError(String(cause));
      }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), running ? 180 : 600);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [sessionId, status]);

  useEffect(() => {
    if (!sessionId || !screenRef.current || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry.contentRect.width < 80 || entry.contentRect.height < 80) return;
      try { fitAddonRef.current?.fit(); } catch { /* panel can be mid-transition */ }
      const cols = terminalRef.current?.cols ?? Math.max(40, Math.floor(entry.contentRect.width / 8));
      const rows = terminalRef.current?.rows ?? Math.max(10, Math.floor(entry.contentRect.height / 18));
      void resizeTerminalSession(sessionId, rows, cols);
    });
    observer.observe(screenRef.current);
    return () => observer.disconnect();
  }, [sessionId]);

  useEffect(() => {
    const screen = screenRef.current;
    if (screen && stickToBottomRef.current) screen.scrollTop = screen.scrollHeight;
  }, [output]);

  async function execute() {
    if (!project || !proposal) return;
    setError(null);
    try {
      if (!sessionId) throw new Error("终端会话尚未连接");
      const accepted = await writeTerminalCommand(project.id, sessionId, proposal, true);
      setStatus(accepted.state); setProposal(null); stickToBottomRef.current = true;
      onCommandAccepted();
    }
    catch (cause) { setError(String(cause)); }
  }
  async function interrupt() {
    if (!sessionId) return;
    setError(null);
    try { const result = await interruptTerminalSession(sessionId); setStatus(result.state); }
    catch (cause) { setError(String(cause)); }
  }
  async function close() {
    if (!sessionId) return;
    setError(null);
    try { await closeTerminalSession(sessionId); setSessionId(null); setStatus("closed"); }
    catch (cause) { setError(String(cause)); }
  }
  return <div className="terminal-workspace">
    <div className="inspector-subtabs"><button className={mode === "shell" ? "selected" : ""} onClick={() => setMode("shell")}>终端</button><button className={mode === "log" ? "selected" : ""} onClick={() => setMode("log")}>执行日志</button></div>
    {mode === "log" ? <div className="terminal-view">{visible.length ? visible.map(({ event, label }) => <div className="terminal-line" key={event.sequence}><time>{new Date(event.createdAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })}</time><span>{label}</span></div>) : <p className="muted">这一轮还没有执行记录。</p>}</div> : <>
      <div className="terminal-screen" ref={screenRef}><div className="terminal-welcome"><span className={`terminal-state ${status}`} />UCode 持久终端 · {project?.name ?? "未选择项目"}</div><div className="terminal-emulator" ref={terminalHostRef} /><pre className="terminal-transcript" aria-hidden="true">{stripAnsi(output)}</pre></div>
      <div className="terminal-toolbar"><div>{["exited", "closed", "failed"].includes(status) ? <button type="button" onClick={() => setRestartNonce((value) => value + 1)}>重新连接</button> : <button type="button" onClick={() => void close()}>关闭会话</button>}{running && <button className="terminal-stop" type="button" onClick={() => void interrupt()} disabled={status === "interrupting"}>{status === "interrupting" ? "正在停止…" : "停止 Ctrl+C"}</button>}</div><span>{status === "running" ? "命令运行中" : status === "interrupting" ? "正在中断" : status === "ready" ? "会话已就绪" : "正在连接"}</span></div>
      <form className="terminal-input" onSubmit={(event) => { event.preventDefault(); if (command.trim()) { setProposal(command.trim()); setCommand(""); } }}><span>›</span><input aria-label="终端命令" value={command} onChange={(event) => setCommand(event.target.value)} placeholder="输入命令，回车后审批" disabled={!project || !sessionId || status !== "ready" || running} /></form>
      {proposal && <div className="inline-approval"><b>批准写入交互终端？</b><code>{proposal}</code><small>工作目录：{project?.path}</small><small>批准后命令以当前用户权限运行。</small><div><button onClick={() => setProposal(null)}>拒绝</button><button className="approve" onClick={() => void execute()} disabled={running}>{running ? "写入中…" : "批准执行"}</button></div></div>}
      {error && <div className="panel-error">{error}</div>}
    </>}
  </div>;
}

function GitWorkspace({ project, snapshot, loading, onChange, onOpenProject }: { project: ProjectSummary | null; snapshot: GitDetails | null; loading: boolean; onChange: (value: GitDetails) => void; onOpenProject: (path: string) => Promise<void> }) {
  const [view, setView] = useState<"changes" | "review" | "history" | "branches" | "worktrees">("changes");
  const [proposal, setProposal] = useState<{ operation: string; paths?: string[]; message?: string; branch?: string } | null>(null);
  const [commitMessage, setCommitMessage] = useState("");
  const [branch, setBranch] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [reviewPath, setReviewPath] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [worktrees, setWorktrees] = useState<WorktreeEntry[] | null>(null);
  const [worktreeName, setWorktreeName] = useState("");
  const [worktreeBranch, setWorktreeBranch] = useState("");
  const [worktreeBase, setWorktreeBase] = useState("HEAD");
  const [worktreeProposal, setWorktreeProposal] = useState<{ kind: "create"; name: string; branch: string; base: string } | { kind: "remove"; path: string; branch: string } | null>(null);
  const [worktreeLoading, setWorktreeLoading] = useState(false);
  useEffect(() => {
    if (view !== "worktrees" || !project || worktrees) return;
    setWorktreeLoading(true); setError(null);
    listWorktrees(project.id).then(setWorktrees).catch((cause) => setError(friendlyGitError(String(cause)))).finally(() => setWorktreeLoading(false));
  }, [view, project?.id, worktrees]);
  if (loading || !snapshot) return <PanelLoading label="正在读取 Git 工作区" />;
  if (!snapshot.isRepository) return <div className="context-placeholder"><span>⑂</span><p>当前项目不是 Git 仓库。UCode 不会自动初始化仓库。</p></div>;
  const changes = snapshot.changes;
  const changedPaths = changes.map((change) => change.path);
  const stagedPaths = changes.filter((change) => change.staged).map((change) => change.path);
  const reviewFiles = splitDiffByFile(snapshot.stagedDiff || snapshot.diff);
  const activeReview = reviewFiles.find((file) => file.path === reviewPath) ?? reviewFiles[0];
  async function apply() {
    if (!project || !proposal) return;
    setError(null);
    try { onChange(await applyGitOperation(project.id, proposal.operation, proposal.paths ?? [], proposal.message ?? "", proposal.branch ?? "", true)); setProposal(null); setCommitMessage(""); setBranch(""); }
    catch (cause) { setError(friendlyGitError(String(cause))); }
  }
  async function refresh() {
    if (!project) return;
    setRefreshing(true); setError(null);
    try { onChange(await readGitDetails(project.id)); }
    catch (cause) { setError(friendlyGitError(String(cause))); }
    finally { setRefreshing(false); }
  }
  async function applyWorktree() {
    if (!project || !worktreeProposal) return;
    setWorktreeLoading(true); setError(null);
    try {
      const next = worktreeProposal.kind === "create"
        ? await createWorktree(project.id, worktreeProposal.name, worktreeProposal.branch, worktreeProposal.base, true)
        : await removeWorktree(project.id, worktreeProposal.path, true);
      setWorktrees(next); setWorktreeProposal(null);
      if (worktreeProposal.kind === "create") { setWorktreeName(""); setWorktreeBranch(""); setWorktreeBase("HEAD"); }
    } catch (cause) { setError(friendlyGitError(String(cause))); }
    finally { setWorktreeLoading(false); }
  }
  return <div className="git-workspace">
    <div className="git-branch"><span>⌁</span><b>{snapshot.branch || "HEAD"}</b><small>{snapshot.clean ? "工作区干净" : `${changes.length} 个变更`}</small><button aria-label="刷新 Git 状态" onClick={() => void refresh()} disabled={refreshing}>{refreshing ? "…" : "↻"}</button></div>
    <div className="inspector-subtabs">{([['changes','变更'],['review','审查'],['history','历史'],['branches','分支'],['worktrees','工作树']] as const).map(([id,label]) => <button className={view === id ? "selected" : ""} onClick={() => setView(id)} key={id}>{label}</button>)}</div>
    {view === "changes" && <div className="git-changes">{changes.map((change) => <div className="git-change-row" key={`${change.path}-${change.status}`}><code><span>{change.status}</span>{change.path}</code><div>{change.unstaged && <button onClick={() => setProposal({ operation: "stage", paths: [change.path] })}>暂存</button>}{change.staged && <button onClick={() => setProposal({ operation: "unstage", paths: [change.path] })}>取消暂存</button>}</div></div>)}{changedPaths.length > 0 && <button onClick={() => setProposal({ operation: "stage", paths: changedPaths })}>暂存全部变更</button>}<div className="git-commit-card"><small>将提交 {stagedPaths.length} 个已暂存文件</small>{stagedPaths.map((path) => <code key={path}>{path}</code>)}<label><span>提交信息</span><input aria-label="Git 提交信息" value={commitMessage} onChange={(event) => setCommitMessage(event.target.value)} /></label><button disabled={!commitMessage.trim() || stagedPaths.length === 0} onClick={() => setProposal({ operation: "commit", message: commitMessage })}>提交已暂存内容</button></div></div>}
    {view === "review" && <div className="git-review-layout">{reviewFiles.length > 0 && <div className="git-review-files" aria-label="审查文件">{reviewFiles.map((file) => <button className={activeReview?.path === file.path ? "selected" : ""} onClick={() => setReviewPath(file.path)} key={file.path}>{file.path}</button>)}</div>}<pre className="git-diff-review">{activeReview?.diff || "没有可审查的差异。"}</pre></div>}
    {view === "history" && <div className="git-history">{snapshot.history.map((line,index) => <code key={index}>{line}</code>)}</div>}
    {view === "branches" && <div className="git-branches">{snapshot.branches.map((line,index) => <button key={index} onClick={() => setProposal({ operation: "switch_branch", branch: line.replace(/^\*\s*/, "").trim() })}>{line}</button>)}<label><span>新分支</span><input value={branch} onChange={(event) => setBranch(event.target.value)} /></label><button disabled={!branch.trim()} onClick={() => setProposal({ operation: "create_branch", branch })}>创建并切换</button></div>}
    {view === "worktrees" && <div className="worktree-view">
      {worktreeLoading && !worktrees ? <PanelLoading label="正在读取 Git Worktree" /> : <div className="worktree-list">{worktrees?.map((entry) => <article className="worktree-card" key={entry.path}><div><b>{entry.branch}</b><span className={entry.dirty ? "dirty" : "clean"}>{entry.dirty ? "有改动" : "干净"}</span></div><code title={entry.path}>{entry.path}</code><small>{entry.head.slice(0, 10)}{entry.isMain ? " · 主工作区" : ""}{entry.locked ? " · 已锁定" : ""}{entry.prunable ? " · 可清理" : ""}</small><div><button onClick={() => void onOpenProject(entry.path)}>在 UCode 中打开</button>{!entry.isMain && <button className="danger" disabled={entry.locked || entry.dirty} title={entry.dirty ? "请先提交或清理未提交改动" : "移除 Worktree（不会删除分支）"} onClick={() => setWorktreeProposal({ kind: "remove", path: entry.path, branch: entry.branch })}>移除</button>}</div></article>)}</div>}
      <div className="worktree-create"><b>新建隔离工作区</b><label><span>目录名称</span><input aria-label="Worktree 目录名称" value={worktreeName} onChange={(event) => setWorktreeName(event.target.value)} placeholder="feature-login" /></label><label><span>分支名称</span><input aria-label="Worktree 分支名称" value={worktreeBranch} onChange={(event) => setWorktreeBranch(event.target.value)} placeholder="feature/login" /></label><label><span>基准</span><input aria-label="Worktree 基准" value={worktreeBase} onChange={(event) => setWorktreeBase(event.target.value)} placeholder="HEAD" /></label><button disabled={!worktreeName.trim() || !worktreeBranch.trim() || worktreeLoading} onClick={() => setWorktreeProposal({ kind: "create", name: worktreeName.trim(), branch: worktreeBranch.trim(), base: worktreeBase.trim() || "HEAD" })}>创建 Worktree</button></div>
    </div>}
    {proposal && <div className="inline-approval"><b>批准 Git 写操作？</b><code>{proposal.operation}{proposal.message ? ` · ${proposal.message}` : ""}{proposal.branch ? ` · ${proposal.branch}` : ""}</code>{proposal.paths?.map((path) => <small key={path}>{path}</small>)}<div><button onClick={() => setProposal(null)}>拒绝</button><button className="approve" onClick={() => void apply()}>批准并执行</button></div></div>}
    {worktreeProposal && <div className="inline-approval"><b>{worktreeProposal.kind === "create" ? "批准创建 Worktree？" : "批准移除 Worktree？"}</b><code>{worktreeProposal.kind === "create" ? `${worktreeProposal.branch} · ${worktreeProposal.name} · 基于 ${worktreeProposal.base}` : `${worktreeProposal.branch} · ${worktreeProposal.path}`}</code>{worktreeProposal.kind === "remove" && <small>只移除工作目录，Git 分支会保留。</small>}<div><button onClick={() => setWorktreeProposal(null)}>拒绝</button><button className="approve" onClick={() => void applyWorktree()}>批准并执行</button></div></div>}
    {error && <div className="panel-error">{error}</div>}
  </div>;
}

function SubagentWorkspace({ ticket, events, onCompleted }: { ticket: TicketSummary; events: RuntimeEvent[]; onCompleted: () => void }) {
  const [assignments, setAssignments] = useState<AssignmentSummary[]>([]);
  const [drafts, setDrafts] = useState<SubagentRequest[]>([{ label: "", request: "", capability: "read" }]);
  const [proposal, setProposal] = useState<SubagentRequest[] | null>(null);
  const [mergeProposal, setMergeProposal] = useState<AssignmentSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const terminalSequence = events.filter((event) => ["subagents.started", "subagents.completed", "subagents.failed", "subagents.cancelled", "subagent.completed", "subagent.failed", "subagent.effect_proposed", "subagent.change_ready", "subagent.change_rejected", "subagent.change_merged"].includes(event.eventType)).at(-1)?.sequence ?? 0;
  const active = assignments.some((assignment) => ["queued", "running", "waiting"].includes(assignment.state));
  const completionReportedRef = useRef(false);

  async function refresh() {
    try { setAssignments(await listSubagents(ticket.id)); setError(null); }
    catch (cause) { setError(String(cause)); }
    finally { setLoading(false); }
  }

  useEffect(() => { setLoading(true); void refresh(); }, [ticket.id, terminalSequence]);
  useEffect(() => {
    if (completionReportedRef.current || !assignments.some((assignment) => assignment.state === "completed")) return;
    completionReportedRef.current = true;
    onCompleted();
  }, [assignments, onCompleted]);
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => void refresh(), 900);
    return () => window.clearInterval(timer);
  }, [ticket.id, active]);

  function updateDraft(index: number, field: keyof SubagentRequest, value: SubagentRequest[keyof SubagentRequest]) {
    setDrafts((current) => current.map((draft, item) => item === index ? { ...draft, [field]: value } : draft));
  }

  async function approve() {
    if (!proposal) return;
    setLoading(true); setError(null);
    try { setAssignments(await startSubagents(ticket.id, proposal, true)); setProposal(null); }
    catch (cause) { setError(String(cause)); }
    finally { setLoading(false); }
  }

  async function stop() {
    setLoading(true); setError(null);
    try { setAssignments(await cancelSubagents(ticket.id)); }
    catch (cause) { setError(String(cause)); }
    finally { setLoading(false); }
  }

  async function resolveEdit(assignmentId: string, approved: boolean) {
    setLoading(true); setError(null);
    try { setAssignments(await resolveSubagentEffect(assignmentId, approved)); }
    catch (cause) { setError(String(cause)); }
    finally { setLoading(false); }
  }

  async function mergeEdit() {
    if (!mergeProposal) return;
    setLoading(true); setError(null);
    try { setAssignments(await mergeSubagentChange(mergeProposal.id)); setMergeProposal(null); }
    catch (cause) { setError(String(cause)); }
    finally { setLoading(false); }
  }

  const children = assignments.filter((assignment) => assignment.parentId);
  const validDrafts = drafts.map((draft) => ({ label: draft.label.trim(), request: draft.request.trim(), capability: draft.capability })).filter((draft) => draft.label && draft.request);
  return <div className="subagent-workspace">
    <div className="subagent-heading"><div><b>并行子任务</b><small>最多 3 个真实本地 Agent · 编辑任务使用独立 Worktree</small></div>{active && <div className="subagent-running"><span><i />运行中</span><button disabled={loading} onClick={() => void stop()}>停止全部</button></div>}</div>
    {children.length > 0 && <div className="subagent-list">{children.map((assignment) => <details className={`subagent-card ${assignment.state}`} key={assignment.id} open={assignment.state !== "completed" || assignment.capability === "edit"}><summary><span className="subagent-state">{assignment.state === "completed" ? "✓" : assignment.state === "failed" ? "!" : assignment.state === "cancelled" ? "×" : "·"}</span><div><b>{assignment.label}</b><small>{assignment.capability === "edit" ? "隔离编辑" : "只读分析"} · {subagentStateLabel(assignment)}</small></div><span>⌄</span></summary><p>{assignment.request}</p>{assignment.worktreePath && <code className="subagent-worktree">独立 Worktree · 主工作区未改变</code>}{assignment.result && <div className="subagent-result">{assignment.result}</div>}{assignment.diff && <pre className="subagent-diff">{assignment.diff}</pre>}{assignment.capability === "edit" && assignment.state === "waiting" && <div className="subagent-edit-actions"><button disabled={loading} onClick={() => void resolveEdit(assignment.id, false)}>拒绝修改</button><button className="approve" disabled={loading} onClick={() => void resolveEdit(assignment.id, true)}>批准写入 Worktree</button></div>}{assignment.capability === "edit" && assignment.state === "completed" && assignment.diff && !assignment.mergedAt && <div className="subagent-edit-actions"><button className="approve" disabled={loading} onClick={() => setMergeProposal(assignment)}>审查并合并</button></div>}{assignment.mergedAt && <div className="subagent-merged">✓ 已合并 · 主工作区仍需自行暂存或提交</div>}{assignment.error && <div className="panel-error">{assignment.error}</div>}</details>)}</div>}
    {!active && <div className="subagent-create"><small>只读任务并发分析；编辑任务在独立 Git Worktree 中生成 Diff，需两次批准才会进入主工作区。</small>{drafts.map((draft, index) => <div className="subagent-draft" key={index}><div><input aria-label={`子任务 ${index + 1} 名称`} value={draft.label} onChange={(event) => updateDraft(index, "label", event.target.value)} placeholder="例如：修复登录校验" />{drafts.length > 1 && <button aria-label={`移除子任务 ${index + 1}`} onClick={() => setDrafts((current) => current.filter((_, item) => item !== index))}>×</button>}</div><div className="subagent-capability" aria-label={`子任务 ${index + 1} 权限`}><button className={draft.capability === "read" ? "selected" : ""} onClick={() => updateDraft(index, "capability", "read")}>只读分析</button><button className={draft.capability === "edit" ? "selected" : ""} onClick={() => updateDraft(index, "capability", "edit")}>隔离编辑</button></div><textarea aria-label={`子任务 ${index + 1} 内容`} value={draft.request} onChange={(event) => updateDraft(index, "request", event.target.value)} placeholder={draft.capability === "edit" ? "说明要修改的现有文件和预期结果。" : "描述一个只读分析问题。"} /></div>)}<div className="subagent-create-actions"><button disabled={drafts.length >= 3} onClick={() => setDrafts((current) => [...current, { label: "", request: "", capability: "read" }])}>+ 添加子任务</button><button className="primary" disabled={!validDrafts.length || loading || ticket.state === "running" || ticket.state === "stopping"} onClick={() => setProposal(validDrafts)}>委派 {validDrafts.length || ""} 个任务</button></div></div>}
    {proposal && <div className="inline-approval"><b>批准启动 {proposal.length} 个 Sub-agent？</b>{proposal.map((request) => <small key={request.label}>{request.capability === "edit" ? "隔离编辑" : "只读"} · {request.label} · {request.request}</small>)}<code>{proposal.some((request) => request.capability === "edit") ? "编辑权限：独立 Worktree · 写入和合并分别审批" : "权限：只读项目分析 · 不允许写文件或执行命令"}</code><div><button onClick={() => setProposal(null)}>拒绝</button><button className="approve" onClick={() => void approve()}>批准并启动</button></div></div>}
    {mergeProposal && <div className="inline-approval"><b>批准合并到主工作区？</b><small>{mergeProposal.label}</small><code>合并前会重新校验 Worktree Diff，并执行 git apply --check；不会自动暂存或提交。</code><div><button onClick={() => setMergeProposal(null)}>取消</button><button className="approve" onClick={() => void mergeEdit()}>批准合并</button></div></div>}
    {loading && !assignments.length && <PanelLoading label="正在读取 Sub-agent 状态" />}
    {error && <div className="panel-error">{error}</div>}
  </div>;
}

function TaskList({ steps }: { steps: TaskStep[] }) {
  return <ul className="task-status" aria-label="任务清单">{steps.map((step) => <li className={step.state} key={step.key}><span className="todo-check">{step.state === "done" ? "✓" : step.state === "failed" ? "!" : step.state === "active" ? "·" : ""}</span><span>{step.label}</span></li>)}</ul>;
}

type TaskStep = { key: string; label: string; state: "done" | "active" | "waiting" | "failed" };

function currentTurnSteps(ticket: TicketSummary | null, events: RuntimeEvent[]): TaskStep[] {
  if (!ticket) return [];
  const start = Math.max(0, events.map((event) => event.eventType).lastIndexOf("user.message"));
  const turn = events.slice(start);
  const observed = (type: string) => turn.some((event) => event.eventType === type);
  const failed = observed("ticket.failed") || ticket.state === "failed";
  const recoveryRequired = observed("effect.recovery_required");
  const waiting = observed("effect.proposed") && !observed("effect.approved") && !observed("effect.rejected") && !recoveryRequired;
  const completed = observed("ticket.completed") || ticket.state === "completed";
  const steps: TaskStep[] = [
    { key: "understand", label: "理解本轮请求", state: observed("assignment.started") ? "done" : failed ? "failed" : "active" },
  ];
  if (observed("workspace.ready")) steps.push({ key: "workspace", label: "读取项目上下文", state: "done" });
  if (observed("provider.started")) steps.push({ key: "model", label: "本地模型生成与检查", state: observed("delivery.ready") || observed("provider.message") || observed("effect.proposed") ? "done" : failed ? "failed" : "active" });
  if (recoveryRequired) steps.push({ key: "recovery", label: "文件恢复需要人工检查", state: "failed" });
  else if (waiting) steps.push({ key: "approval", label: "等待批准文件修改", state: "active" });
  else if (observed("effect.approved")) steps.push({ key: "approval", label: "执行已批准的修改", state: completed ? "done" : failed ? "failed" : "active" });
  else if (observed("effect.rejected")) steps.push({ key: "approval", label: "修改已拒绝", state: "done" });
  if (completed) steps.push({ key: "complete", label: "本轮任务完成", state: "done" });
  if (failed) steps.push({ key: "failed", label: "本轮执行失败", state: "failed" });
  return steps;
}

function PanelLoading({ label }: { label: string }) { return <div className="panel-loading"><span /><p>{label}</p></div>; }
function count(value: string) { return value ? value.split(/\r?\n/).length : 0; }
function formatBytes(value: number) { return value < 1024 ? `${value} B` : `${Math.round(value / 1024)} KB`; }
function eventLabel(type: string) { return ({ "assignment.started": "开始处理请求", "workspace.ready": "已载入工作区", "provider.started": "调用本地模型", "tool.completed": "完成项目操作", "provider.reconnecting": "模型连接恢复中", "provider.reconnected": "模型连接已恢复", "effect.proposed": "生成待审批修改", "effect.approved": "已批准修改", "effect.rejected": "已拒绝修改", "effect.recovery_required": "文件恢复需要人工检查", "delivery.ready": "回答已生成", "ticket.completed": "任务完成", "ticket.failed": "任务失败", "ticket.cancelled": "任务已停止", "subagents.started": "已启动并行任务", "subagents.completed": "并行任务已完成", "subagents.failed": "并行任务失败", "subagents.cancelled": "并行任务已停止" } as Record<string, string>)[type] ?? null; }
function stripAnsi(value: string) { return value.replace(/[\u001B\u009B][[\]()#;?]*(?:(?:(?:[a-zA-Z\d]*(?:;[-a-zA-Z\d\/#&.:=?%@~_]+)*)?\u0007)|(?:(?:\d{1,4}(?:[;:]\d{0,4})*)?[\dA-PR-TZcf-nq-uy=><~]))/g, ""); }

function splitDiffByFile(diff: string): { path: string; diff: string }[] {
  if (!diff.trim()) return [];
  const chunks = diff.split(/(?=^diff --git )/m).filter(Boolean);
  return chunks.map((chunk, index) => {
    const match = chunk.match(/^diff --git a\/(.+?) b\/(.+)$/m);
    return { path: match?.[2] ?? `变更 ${index + 1}`, diff: chunk.trim() };
  });
}

function friendlyGitError(value: string) {
  const normalized = value.toLowerCase();
  if (normalized.includes("local changes") || normalized.includes("would be overwritten")) return "当前改动会被分支切换覆盖。请先提交或暂存改动；UCode 没有自动丢弃任何文件。";
  if (normalized.includes("conflict") || normalized.includes("unmerged")) return "仓库存在未解决冲突。请在审查视图确认冲突文件，解决后逐个暂存；UCode 不会自动选择任一版本。";
  if (normalized.includes("nothing to commit") || normalized.includes("no changes added")) return "当前没有已暂存的内容。请先在“变更”中暂存需要提交的文件。";
  if (normalized.includes("identity unknown") || normalized.includes("user.email")) return "Git 身份尚未配置。请在设置 → Git 中填写姓名和邮箱后重试。";
  return value;
}

function subagentStateLabel(assignment: AssignmentSummary) {
  if (assignment.capability === "edit") {
    if (assignment.state === "running") return "正在隔离工作区生成修改";
    if (assignment.state === "waiting") return "等待批准写入隔离工作区";
    if (assignment.state === "completed" && assignment.diff && !assignment.mergedAt) return "Diff 已就绪，等待合并审批";
    if (assignment.mergedAt) return "已合并，尚未暂存";
  }
  return ({ queued: "等待调度", running: "正在分析", waiting: "等待输入", completed: "分析完成", failed: "执行失败", cancelled: "已取消" } as const)[assignment.state];
}
