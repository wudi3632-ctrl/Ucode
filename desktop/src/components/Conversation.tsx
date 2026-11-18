import { FormEvent, KeyboardEvent, useEffect, useMemo, useRef, useState } from "react";
import { chooseContextAttachments, removeContextAttachment } from "../bridge/desktop";
import type { AppliedEffect, AppSettings, ContextAttachmentSelection, ContextAttachmentSummary, EvidenceReceipt, OnboardingState, ProposedEffect, R1ProbeProposal, R1ProbeRecord, RaceBundle, RaceCandidate, RescueRepairProposal, RescueRepairRecord, RuntimeEvent, TicketSummary } from "../domain/types";
import type { ObservedModelPerformance } from "../domain/modelPerformance";
import { changedLineCounts } from "../domain/diff";
import { buildComposerHistory, canStartComposerHistory, exitComposerHistory, moveComposerHistory, type ComposerHistoryState } from "../domain/composerHistory";
import { RichText } from "./RichText";
import { completedOnboardingState, OnboardingTip, OnboardingWelcome } from "./Onboarding";

interface ConversationProps {
  projectId: string | null;
  ticket: TicketSummary | null;
  events: RuntimeEvent[];
  effect: ProposedEffect | null;
  receipt: EvidenceReceipt | null;
  appliedEffect: AppliedEffect | null;
  busy: boolean;
  races?: RaceBundle[];
  onCancelRace?: (raceId: string) => Promise<void>;
  onSubmit: (request: string, selection?: ContextAttachmentSelection | null, clientRequestId?: string) => Promise<boolean>;
  onCancel: () => Promise<void>;
  onRetry: () => Promise<void>;
  onExport: () => Promise<void>;
  onBranch: (sequence: number) => Promise<void>;
  onApprove: (selectedPaths: string[]) => void;
  onReject: () => void;
  onRollback: () => Promise<void>;
  onResolveRescueRepair?: (proposalId: string, approved: boolean) => Promise<void>;
  onRollbackRescueRepair?: (proposalId: string) => Promise<void>;
  onResolveRescueProbe?: (proposalId: string, approved: boolean) => Promise<void>;
  onExportReceipt: (receipt: EvidenceReceipt, format: "json" | "md") => Promise<void>;
  hasProject: boolean;
  onOpenProject: () => void | Promise<void>;
  onboarding?: OnboardingState;
  onboardingBusy?: boolean;
  onCreateDemo?: () => Promise<void>;
  onDismissOnboarding?: () => Promise<void>;
  onDismissOnboardingTip?: (tipId: string) => Promise<void>;
  onOpenSettings?: () => void;
  error: string | null;
  sidebarOpen: boolean;
  inspectorOpen: boolean;
  onToggleSidebar: () => void;
  onToggleInspector: () => void;
  providerStatus: "offline" | "ready" | "unavailable";
  modelProvider: AppSettings["modelProvider"];
  providerModel: string;
  modelPerformance: ObservedModelPerformance | null;
  providerRecovering?: boolean;
  onRecoverProvider?: () => Promise<void>;
}

type TimelineItem =
  | { key: string; kind: "user"; content: string; sequence: number; attachments: ContextAttachmentSummary[] }
  | { key: string; kind: "assistant"; content: string; streaming: boolean; sequence: number; source: "stream" | "message" | "delivery" | "effect" }
  | { key: string; kind: "activity"; events: RuntimeEvent[]; active: boolean }
  | { key: string; kind: "recovery"; event: RuntimeEvent; paths: string[] }
  | { key: string; kind: "rescue"; recovery: TaskRescue };

interface TaskRescue {
  reason: "candidate_no_change" | "candidate_no_effect";
  path: string;
  attempts: number;
  actions: Array<"attach_context" | "configure_model">;
}

const activityCopy: Record<string, string> = {
  "tool.completed": "已完成项目读取",
  "rescue.probe.completed": "已完成一项系统只读探针",
  "rescue.diagnosis.ready": "只读诊断证据已汇总",
  "subagents.started": "正在并行分析项目",
  "subagent.started": "已启动只读分析",
  "subagent.completed": "只读分析已完成",
  "subagent.failed": "一项只读分析未完成",
  "subagents.completed": "并行分析已汇总",
  "subagents.failed": "并行分析部分失败",
  "provider.reconnecting": "连接中断，正在恢复生成",
  "provider.reconnected": "连接已恢复，缺失内容已补齐",
};

export function Conversation({ projectId, ticket, events, effect, receipt, appliedEffect, busy, races = [], onCancelRace = async () => undefined, onSubmit, onCancel, onRetry, onExport, onBranch, onApprove, onReject, onRollback, onResolveRescueRepair = async () => undefined, onRollbackRescueRepair = async () => undefined, onResolveRescueProbe = async () => undefined, onExportReceipt, hasProject, onOpenProject, onboarding = completedOnboardingState, onboardingBusy = false, onCreateDemo = async () => undefined, onDismissOnboarding = async () => undefined, onDismissOnboardingTip = async () => undefined, onOpenSettings = () => undefined, error, sidebarOpen, inspectorOpen, onToggleSidebar, onToggleInspector, providerStatus, modelProvider, providerModel, modelPerformance, providerRecovering = false, onRecoverProvider = async () => undefined }: ConversationProps) {
  const [request, setRequest] = useState("");
  const [selection, setSelection] = useState<ContextAttachmentSelection | null>(null);
  const [attachmentLoading, setAttachmentLoading] = useState(false);
  const [modelMenuOpen, setModelMenuOpen] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const [rollbackConfirm, setRollbackConfirm] = useState(false);
  const [effectSelection, setEffectSelection] = useState<{ effectId: string; paths: string[] } | null>(null);
  const [historyState, setHistoryState] = useState<ComposerHistoryState>({ cursor: null, draft: "" });
  const endRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const submittingRef = useRef(false);
  const ticketEvents = useMemo(() => events.filter((event) => event.ticketId === ticket?.id), [events, ticket?.id]);
  const items = useMemo(() => buildTimeline(ticket, ticketEvents, busy), [ticket, ticketEvents, busy]);
  const requestHistory = useMemo(() => buildComposerHistory(items.flatMap((item) => item.kind === "user" ? [item.content] : [])), [items]);
  const taskContext = useMemo(() => latestTaskContext(ticketEvents), [ticketEvents]);
  const rescueRepair = useMemo(() => rescueRepairFrom(ticketEvents), [ticketEvents]);
  const rescueProbe = useMemo(() => rescueProbeFrom(ticketEvents), [ticketEvents]);
  const recoveryRequired = items.some((item) => item.kind === "recovery");
  const latestUserRequest = useMemo(() => [...items].reverse().find((item) => item.kind === "user")?.content ?? ticket?.request ?? "", [items, ticket?.request]);

  useEffect(() => {
    if (typeof endRef.current?.scrollIntoView === "function") {
      endRef.current.scrollIntoView({ behavior: "smooth", block: "end" });
    }
  }, [items.length, busy, effect]);

  useEffect(() => {
    const input = inputRef.current;
    if (!input) return;
    input.style.height = "0px";
    input.style.height = `${Math.min(input.scrollHeight, 164)}px`;
  }, [request]);

  useEffect(() => {
    setSelection(null);
    setRollbackConfirm(false);
    setHistoryState({ cursor: null, draft: "" });
  }, [projectId, ticket?.id]);

  async function submit(event?: FormEvent) {
    event?.preventDefault();
    const value = request.trim();
    if (!value || busy || submittingRef.current) return;
    submittingRef.current = true;
    const submittedSelection = selection;
    const clientRequestId = crypto.randomUUID();
    setRequest("");
    setSelection(null);
    setHistoryState({ cursor: null, draft: "" });
    try {
      const accepted = await onSubmit(value, submittedSelection, clientRequestId);
      if (!accepted) {
        setRequest((current) => current || value);
        setSelection((current) => current || submittedSelection);
        setHistoryState({ cursor: null, draft: value });
      }
    } finally {
      submittingRef.current = false;
    }
  }

  function keyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (event.key === "ArrowUp" && (historyState.cursor !== null || canStartComposerHistory(request, event.currentTarget.selectionStart, event.currentTarget.selectionEnd))) {
      const next = moveComposerHistory(requestHistory, historyState, request, "older");
      if (next) {
        event.preventDefault();
        applyHistory(next);
      }
      return;
    }
    if (event.key === "ArrowDown" && historyState.cursor !== null) {
      const next = moveComposerHistory(requestHistory, historyState, request, "newer");
      if (next) {
        event.preventDefault();
        applyHistory(next);
      }
      return;
    }
    if (event.key === "Escape") {
      const next = exitComposerHistory(historyState);
      if (next) {
        event.preventDefault();
        applyHistory(next);
      }
      return;
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void submit();
    }
  }

  function applyHistory(next: ReturnType<typeof moveComposerHistory> & object) {
    setHistoryState({ cursor: next.cursor, draft: next.draft });
    setRequest(next.value);
    window.setTimeout(() => {
      const input = inputRef.current;
      if (!input) return;
      input.setSelectionRange(next.value.length, next.value.length);
    }, 0);
  }

  async function addAttachments() {
    if (!projectId || attachmentLoading) return;
    setAttachmentLoading(true);
    try {
      const selected = selection
        ? await chooseContextAttachments(projectId, selection.selectionId)
        : await chooseContextAttachments(projectId);
      if (selected) setSelection(selected);
    } finally {
      setAttachmentLoading(false);
      window.setTimeout(() => inputRef.current?.focus(), 0);
    }
  }

  async function removeAttachment(attachmentId: string) {
    if (!selection) return;
    try {
      await removeContextAttachment(selection.selectionId, attachmentId);
      setSelection((current) => {
        if (!current || current.selectionId !== selection.selectionId) return current;
        const attachments = current.attachments.filter((item) => item.id !== attachmentId);
        return attachments.length ? { ...current, attachments } : null;
      });
    } catch (cause) {
      setSelection((current) => current ? { ...current, errors: [...current.errors, { displayName: "附件", code: "attachment_remove_failed", message: cause instanceof Error ? cause.message : String(cause) }] } : current);
    }
  }

  async function copyText(key: string, content: string) {
    await navigator.clipboard.writeText(content);
    setCopied(key);
    window.setTimeout(() => setCopied((current) => current === key ? null : current), 1200);
  }

  const safeConflict = Boolean(error && (error.includes("已拒绝回滚") || error.includes("拒绝整个批次")));
  const selectedEffectPaths = effect
    ? effectSelection?.effectId === effect.id ? effectSelection.paths : effect.changes.map((change) => change.path)
    : [];

  return (
    <main className="conversation">
      <header className="conversation-header">
        <button className={`panel-toggle left-toggle ${sidebarOpen ? "is-open" : ""}`} onClick={onToggleSidebar} aria-label={sidebarOpen ? "收起侧栏" : "展开侧栏"} aria-pressed={sidebarOpen}>
          <span className="panel-toggle-glyph" />
        </button>
        <div className="conversation-title">
          <h1>{ticket?.title ?? "新任务"}</h1>
          <p><span className="branch-icon">⌁</span> {ticket?.parentTicketId ? "对话分支" : "main"} <span>·</span> {busy ? "Agent 正在工作" : "工作区就绪"}</p>
        </div>
        <div className="header-actions">
          {ticket && <button className="header-text-action" onClick={() => void onExport()} title="导出当前对话为 Markdown">导出</button>}
          <button className={`panel-toggle right-toggle ${inspectorOpen ? "is-open" : ""}`} onClick={onToggleInspector} aria-label={inspectorOpen ? "收起任务面板" : "展开任务面板"} aria-pressed={inspectorOpen}>
            <span className="panel-toggle-glyph" />
          </button>
        </div>
      </header>

      <section className="timeline" aria-live="polite" aria-label="对话内容">
        {ticket ? (
          <div className="thread">
            {items.map((item) => {
              if (item.kind === "user") return <UserMessage key={item.key} content={item.content} attachments={item.attachments} copied={copied === item.key} onCopy={() => copyText(item.key, item.content)} onEdit={() => { setRequest(item.content); inputRef.current?.focus(); }} onBranch={() => onBranch(item.sequence)} />;
              if (item.kind === "activity") return <ActivityBlock key={item.key} events={item.events} active={item.active} />;
              if (item.kind === "recovery") return <RecoveryRequiredCard key={item.key} event={item.event} paths={item.paths} />;
              if (item.kind === "rescue") return <TaskRescueRow key={item.key} recovery={item.recovery} onAttachContext={() => void addAttachments()} onConfigureModel={onOpenSettings} />;
              return <AssistantMessage key={item.key} content={item.content} streaming={item.streaming} copied={copied === item.key} onCopy={() => copyText(item.key, item.content)} onRetry={() => void onSubmit(latestUserRequest)} onBranch={() => onBranch(item.sequence)} />;
            })}
            {races.map((race) => <CandidateRaceCard key={race.race.id} bundle={race} onCancel={() => onCancelRace(race.race.id)} />)}
            {rescueProbe && <R1NetworkProbeCard value={rescueProbe} busy={busy} onResolve={onResolveRescueProbe} />}
            {rescueRepair && <RescueRepairCard value={rescueRepair} busy={busy} onResolve={onResolveRescueRepair} onRollback={onRollbackRescueRepair} />}
            {effect && (
              <article className="approval-card">
                <div className="approval-icon">±</div>
                <div className="approval-content">
                  <div className="card-heading"><b>{effect.changes.length} 个文件变更</b><span>需要批准</span></div>
                  <div className="effect-file-selection" aria-label="选择要批准的文件">
                    {effect.changes.map((change) => {
                      const checked = selectedEffectPaths.includes(change.path);
                      const counts = changedLineCounts(change.before, change.after);
                      return <label key={change.path}><input type="checkbox" checked={checked} disabled={effect.atomic} onChange={() => setEffectSelection({ effectId: effect.id, paths: checked ? selectedEffectPaths.filter((path) => path !== change.path) : [...selectedEffectPaths, change.path] })} /><span><b>{change.path}</b><small>{change.operation === "create" ? "新建" : change.operation === "delete" ? "删除" : "修改"}</small></span><div className="change-summary"><span className="addition">+{counts.additions}</span><span className="deletion">−{counts.deletions}</span></div></label>;
                    })}
                  </div>
                  <p>{effect.atomic ? "该候选方案已经整体验证，必须整批批准；写入前仍会重新校验全部文件。" : "批准前会重新校验所有选中文件；任一文件发生变化，整批都不会写入。"}</p>
                  <div className="approval-actions">
                    <button className="quiet" onClick={onReject}>拒绝</button>
                    <button className="approve" disabled={!selectedEffectPaths.length} onClick={() => onApprove(selectedEffectPaths)}>批准选中的 {selectedEffectPaths.length} 个文件</button>
                  </div>
                </div>
              </article>
            )}
            {taskContext && <TaskContextSummary context={taskContext} />}
            {receipt && !busy && <><OnboardingTip id="evidence" title="证据只记录真实结果" dismissed={onboarding.dismissedTips.includes("evidence")} onDismiss={onDismissOnboardingTip}>来源、命令、测试和变更分别列出；没有执行的计划不会伪装成完成。</OnboardingTip><EvidenceReceiptCard receipt={receipt} onExport={onExportReceipt} /></>}
            {appliedEffect && !effect && !busy && (
              <article className="rollback-card">
                <div><b>可恢复的修改</b><span>{appliedEffect.paths.length === 1 ? appliedEffect.paths[0] : `${appliedEffect.paths.length} 个文件`}</span></div>
                {rollbackConfirm ? <div className="rollback-confirm"><p>回滚前会重新核对全部已应用文件；任一文件后来被修改，整批回滚都会停止。</p><button type="button" className="quiet" onClick={() => setRollbackConfirm(false)}>取消</button><button type="button" className="danger" onClick={() => void onRollback().finally(() => setRollbackConfirm(false))}>确认回滚</button></div> : <button type="button" className="quiet" onClick={() => setRollbackConfirm(true)}>回滚这批修改</button>}
              </article>
            )}
            {error && !recoveryRequired && <article className="error-card"><b>{safeConflict ? "文件冲突已安全停止" : "任务未能继续"}</b><p>{error}</p><div><small>{safeConflict ? "当前文件未被覆盖，可在 Diff 中审查后拒绝旧批次并重新发起修改。" : "已保留当前对话和部分回答。"}</small>{!safeConflict && <button type="button" onClick={() => void onRetry()} disabled={busy}>重试本轮</button>}</div></article>}
            <div ref={endRef} />
          </div>
        ) : onboarding.welcomeState === "new" ? (
          <OnboardingWelcome busy={onboardingBusy} onDemo={onCreateDemo} onProject={async () => { await onOpenProject(); }} onDismiss={onDismissOnboarding} />
        ) : (
          <div className="empty-state">
            <div className="empty-glyph"><span>⌘</span></div>
            <h2>{hasProject ? "开始一段新对话" : "先选择一个项目"}</h2>
            <p>{hasProject ? "可以提问、分析代码，或交代一个需要执行的任务。" : "授权项目目录后，UCode 才能读取文件和运行工具。"}</p>
            {!hasProject && <button className="primary empty-action" onClick={onOpenProject}>选择项目目录</button>}
          </div>
        )}
      </section>

      <form className="composer" onSubmit={submit}>
        {providerStatus !== "ready" && <RuntimeRecoveryStrip status={providerStatus} recovering={providerRecovering} onRecover={onRecoverProvider} />}
        {selection && (selection.attachments.length > 0 || selection.errors.length > 0) && <div className="attachment-area">
          {selection.attachments.length > 0 && <div className="attachment-strip" aria-label="已添加上下文">
            {selection.attachments.map((attachment) => <AttachmentChip key={attachment.id} attachment={attachment} removable onRemove={() => void removeAttachment(attachment.id)} />)}
          </div>}
          {selection.errors.map((item) => <div className="attachment-error" role="status" key={`${item.displayName}-${item.code}`}><b>{item.displayName}</b><span>{item.message}</span></div>)}
        </div>}
        <div className="composer-input-wrap">
          <textarea
            ref={inputRef}
            aria-label="任务输入"
            placeholder={recoveryRequired ? "该任务已锁定，请先检查恢复文件…" : ticket ? "继续追问或交代任务…" : "描述你想完成的任务…"}
            value={request}
            onChange={(event) => {
              setRequest(event.target.value);
              if (historyState.cursor !== null) setHistoryState({ cursor: null, draft: event.target.value });
            }}
            onKeyDown={keyDown}
            rows={1}
            disabled={!hasProject || providerStatus !== "ready" || recoveryRequired}
          />
          {historyState.cursor !== null && <span className="composer-history-indicator" role="status">历史 {historyState.cursor + 1}/{requestHistory.length}</span>}
        </div>
        <div className="composer-footer">
          <div className="composer-tools">
            <span className="attachment-tool">
              <button type="button" className="tool-button" aria-label={attachmentLoading ? "正在添加上下文文件" : "添加上下文文件"} aria-busy={attachmentLoading} title="添加上下文文件（最多 8 个 UTF-8 文本）" onClick={addAttachments} disabled={!projectId || attachmentLoading}>{attachmentLoading ? "…" : "＋"}</button>
              <span className="attachment-tool-tip" role="tooltip">最多 8 个 UTF-8 文本文件</span>
            </span>
            <div className="model-control">
              <button type="button" className="model-button" aria-expanded={modelMenuOpen} onClick={() => setModelMenuOpen((value) => !value)}>{modelProvider === "bundled" ? "内置模型" : providerModel || "外部模型"} <span>⌄</span></button>
              {modelMenuOpen && <div className="model-menu" role="dialog" aria-label="模型状态">
                <div className="model-menu-heading"><b>{modelProvider === "bundled" ? "本地推理" : "混合推理"}</b><span className={providerStatus === "ready" ? "healthy" : "unhealthy"}>{providerStatus === "ready" ? "已连接" : "不可用"}</span></div>
                <div className="model-entry"><span><b>UCode 安全路由器</b><small>确定性意图、权限与审批策略</small></span><i /></div>
                <div className="model-entry"><span><b>{modelProvider === "bundled" ? "已验证的本地模型包" : providerModel || "OpenAI 兼容模型"}</b><small>问答、解释与代码生成</small></span><i /></div>
                <ModelPerformanceRow value={modelPerformance} />
                <p>{modelProvider === "bundled" ? "模型在本机运行，项目内容不会发送到云端。" : "回答和代码上下文会发送到已配置端点；工具与审批仍由本机 UCode 控制。"}</p>
              </div>}
            </div>
            <span className={`provider-state ${busy || providerStatus === "offline" ? "active" : ""}`}>{busy ? "生成中" : providerStatus === "ready" ? (modelProvider === "bundled" ? "离线可用" : "端点可用") : providerStatus === "offline" ? "正在启动本地服务" : "服务启动失败"}</span>
          </div>
          <button className={`send ${busy ? "working stop" : ""}`} type={busy ? "button" : "submit"} onClick={busy ? () => void onCancel() : undefined} aria-label={busy ? "停止生成" : "发送任务"} title={busy ? "停止本地模型生成和后续工具执行" : recoveryRequired ? "该任务需要人工恢复，不能继续写入" : undefined} disabled={!busy && (!hasProject || providerStatus !== "ready" || recoveryRequired || !request.trim())}>
            {busy ? <span className="stop-glyph" /> : "↑"}
          </button>
        </div>
      </form>
    </main>
  );
}

function CandidateRaceCard({ bundle, onCancel }: { bundle: RaceBundle; onCancel: () => Promise<void> }) {
  const { race, candidates } = bundle;
  const winnerId = race.verdict?.winnerId ?? null;
  const running = ["prepared", "generating", "validating", "decided"].includes(race.state);
  const state = ({
    prepared: "正在准备隔离区", generating: "正在生成两个方案", validating: "正在本地验证", decided: "正在整理结论",
    awaiting_review: "验证完成，等待审查", no_winner: "没有安全赢家", applied: "推荐方案已应用", rolled_back: "已安全回滚", rejected: "已拒绝",
    stale: "项目已变化，需要重新生成", cancelled: "已停止",
  } as const)[race.state];
  const passed = candidates.filter((candidate) => candidate.state === "passed").length;
  const title = race.state === "no_winner" ? "两个方案均未通过" : winnerId ? "本地验证已选出推荐方案" : "正在比较两个隔离方案";
  return <article className={`candidate-race-card state-${race.state}`} aria-label="候选验证">
    <header>
      <span className="race-mark" aria-hidden="true">◇</span>
      <div><b>{title}</b><small>{state}</small></div>
      <span className={`race-state ${running ? "active" : ""}`}>{running ? "验证中" : `${passed}/${candidates.length} 通过`}</span>
    </header>
    <div className="race-candidates">
      {candidates.map((candidate, index) => <CandidateSummary key={candidate.id} candidate={candidate} label={index === 0 ? "最小改动" : "兼容优先"} winner={candidate.id === winnerId} />)}
    </div>
    {race.state === "no_winner" && <p className="race-explanation">UCode 没有把失败方案包装成成功。原项目保持不变，可展开查看未通过的检查。</p>}
    {race.state === "stale" && <p className="race-explanation">生成后项目内容发生了变化，为避免覆盖新修改，旧结果已失效。</p>}
    <footer>
      <span>{race.allowedPaths.length} 个目标文件 · {race.acceptance.join(" · ")}</span>
      {running && <button type="button" className="quiet" onClick={() => void onCancel()}>停止比较</button>}
    </footer>
  </article>;
}

function CandidateSummary({ candidate, label, winner }: { candidate: RaceCandidate; label: string; winner: boolean }) {
  const failed = candidate.facts.filter((fact) => fact.outcome === "failed").length;
  const passed = candidate.facts.filter((fact) => fact.outcome === "passed").length;
  const state = ({ queued: "等待", generating: "生成中", proposed: "已生成", validating: "验证中", passed: "通过", failed: "未通过", rejected: "未采用", cancelled: "已停止" } as const)[candidate.state];
  return <details className={`race-candidate ${winner ? "winner" : ""}`}>
    <summary>
      <span className="candidate-index" aria-hidden="true">{winner ? "✓" : ""}</span>
      <span><b>{label}</b><small>{winner ? "推荐 · " : ""}{state}</small></span>
      <span className="candidate-metrics">{candidate.metrics.modifiedFiles || 0} 文件 <i>+−{candidate.metrics.changedLines || 0}</i></span>
      <span className="candidate-chevron">⌄</span>
    </summary>
    <div className="candidate-details">
      <div className="candidate-fact-summary"><span className="fact-pass">{passed} 通过</span><span className={failed ? "fact-fail" : "fact-muted"}>{failed} 失败</span><span>{candidate.metrics.deletedFiles} 删除</span></div>
      {candidate.facts.length > 0 ? <ul>{candidate.facts.map((fact) => <li key={fact.checkId} className={`fact-${fact.outcome}`}><span>{fact.outcome === "passed" ? "✓" : fact.outcome === "failed" ? "×" : "–"}</span><div><b>{fact.summary}</b>{fact.command && <code>{fact.command}{fact.exitCode === null ? "" : ` · exit ${fact.exitCode}`}</code>}</div></li>)}</ul> : <p>验证事实将在本地检查完成后出现。</p>}
      {candidate.diff && <details className="candidate-diff"><summary>查看候选 Diff</summary><pre>{candidate.diff}</pre></details>}
    </div>
  </details>;
}

function RuntimeRecoveryStrip({ status, recovering, onRecover }: { status: "offline" | "unavailable"; recovering: boolean; onRecover: () => Promise<void> }) {
  const starting = status === "offline";
  return <div className={`runtime-recovery-strip ${starting ? "starting" : "failed"}`} role="status">
    <span className="runtime-recovery-indicator" />
    <div><b>{starting ? "本地模型正在启动" : "本地模型服务未响应"}</b><small>{starting ? "首次启动可能需要一些时间，对话和项目内容仍保存在本机。" : "当前对话已保留，可以安全地重新启动本地服务。"}</small></div>
    {!starting && <button type="button" className="quiet" disabled={recovering} onClick={() => void onRecover()}>{recovering ? "恢复中…" : "恢复本地服务"}</button>}
  </div>;
}

function ModelPerformanceRow({ value }: { value: ObservedModelPerformance | null }) {
  const outcome = value ? ({ generating: "生成中", completed: "已完成", cancelled: "已停止", failed: "失败" } as const)[value.outcome] : null;
  const measurements = value ? [
    value.firstCharacterMs === null ? null : `首字 ${formatLatency(value.firstCharacterMs)}`,
    value.charactersPerSecond === null ? null : `输出 ${value.charactersPerSecond.toFixed(1)} 字/秒`,
    value.characterCount > 0 ? `${value.characterCount} 字` : null,
  ].filter((item): item is string => Boolean(item)) : [];
  return <section className={`model-performance ${value?.active ? "active" : ""}`} aria-label="最近模型性能">
    <div><b>最近响应{outcome ? ` · ${outcome}` : ""}</b>{value?.active && <span aria-hidden="true"><i /><i /><i /></span>}</div>
    {measurements.length > 0
      ? <p className="model-performance-values">{measurements.map((item) => <span key={item}>{item}</span>)}</p>
      : <p className="model-performance-empty">{value?.active ? "正在等待首字…" : "尚无可测量的回答"}</p>}
    <small>端到端观测，包含推理与本地传输</small>
  </section>;
}

function formatLatency(milliseconds: number) {
  return milliseconds < 1000 ? `${Math.round(milliseconds)} 毫秒` : `${(milliseconds / 1000).toFixed(1)} 秒`;
}

function EvidenceReceiptCard({ receipt, onExport }: { receipt: EvidenceReceipt; onExport: (receipt: EvidenceReceipt, format: "json" | "md") => Promise<void> }) {
  const legacyTests = receipt.sources.filter(isTestEvidence);
  const visibleSources = receipt.sources.filter((item) => !isTestEvidence(item));
  const visibleTests = [...receipt.tests, ...legacyTests];
  const total = visibleSources.length + receipt.commands.length + visibleTests.length + receipt.effects.length + receipt.decisions.length;
  const appliedEffects = receipt.effects.filter((effect) => effect.event === "effect.applied");
  const rescueRepair = [...receipt.effects].reverse().find((effect) => typeof effect.event === "string" && effect.event.startsWith("rescue.repair.") && effect.event !== "rescue.repair.proposed");
  const rescueRepairEvent = String(rescueRepair?.event ?? "");
  const rescueVerified = rescueRepairEvent === "rescue.repair.verified";
  const rescueRolledBack = rescueRepairEvent === "rescue.repair.rolled_back";
  const rescueUnverified = rescueRepairEvent === "rescue.repair.unverified";
  const rescueRollbackFailed = rescueRepairEvent === "rescue.repair.rollback_failed";
  const rejected = appliedEffects.length === 0 && receipt.decisions.some((decision) => decision.event === "effect.rejected");
  const noWinnerDecision = receipt.decisions.find((decision) => decision.event === "race.decided" && nestedValue(decision, ["detail", "race", "verdict", "kind"]) === "no_winner");
  const legacyRaceStopped = receipt.decisions.find((decision) => decision.event === "race.legacy_stopped");
  const staleRaceReconciled = receipt.decisions.find((decision) => decision.event === "race.terminal_reconciled" && nestedValue(decision, ["detail", "raceState"]) === "stale");
  const outcome = rejected
    ? "已取消"
    : rescueRolledBack
      ? "已回滚"
      : rescueVerified
        ? "写入已验证"
        : rescueUnverified || rescueRollbackFailed
          ? "需要检查"
          : ({ completed: "已完成", failed: "失败", cancelled: "已停止", rolled_back: "已回滚", waiting_input: "等待输入", waiting_approval: "等待审批", in_progress: "进行中" } as Record<string, string>)[receipt.outcome] ?? receipt.outcome;
  const changed = appliedEffects.length
    ? `${appliedEffects.length} 个已应用批次`
    : rescueRolledBack
      ? "用户级系统修改已精确回滚"
      : rescueVerified
        ? "已应用 1 个用户级系统修复"
        : rescueUnverified
          ? "系统写入已发生，但尚未验证"
          : receipt.route === "rescue"
            ? "没有修改系统"
            : "没有写入文件";
  const verified = rejected
    ? "未执行测试（修改已拒绝）"
    : rescueRolledBack
      ? "回滚后已重读目标并确认恢复审批前原值"
    : rescueVerified
      ? "修复后已重读目标并核对精确摘要"
    : rescueUnverified
      ? "写入后的目标重读验证未通过"
    : visibleTests.length
    ? legacyRaceStopped
      ? `仅旧候选验证：${summarizeTestEvidence(visibleTests)}；任务已在写入前安全失效`
      : staleRaceReconciled
        ? `仅候选验证：${summarizeTestEvidence(visibleTests)}；项目已变化，结果未写入`
        : summarizeTestEvidence(visibleTests)
    : noWinnerDecision
      ? String(nestedValue(noWinnerDecision, ["detail", "testSkippedReason"]) ?? "机械验收未执行：候选未通过写入前完整性检查")
      : legacyRaceStopped
        ? "机械验收未执行：旧任务缺少安全操作约束，已在写入前终止"
    : visibleSources.length
      ? receipt.route === "rescue" ? `${visibleSources.length} 项系统只读证据已核验` : `${visibleSources.length} 项项目来源已核验`
      : appliedEffects.length
        ? "尚未记录测试结果"
        : receipt.route === "chat" || receipt.route === "code_generate"
          ? "本轮不需要项目证据"
          : "本轮不需要运行测试";
  const risk = rejected
    ? "修改已拒绝，项目文件未改变"
    : rescueRolledBack
      ? "系统已恢复到审批前状态"
    : rescueVerified
      ? "系统已修改；仅当目标仍保持应用后状态时允许回滚"
    : rescueUnverified
      ? "系统可能已修改但验证未通过，需要人工核对当前状态"
    : rescueRollbackFailed
      ? "回滚失败或被安全拒绝，需要人工核对当前系统状态"
    : receipt.outcome === "completed"
    ? appliedEffects.length
      ? "文件已变更，仍应审查 Diff 和测试结果"
      : visibleSources.length
        ? "回答范围仅限于本轮实际读取的项目内容"
        : receipt.route === "chat" || receipt.route === "code_generate"
          ? "未读取项目文件；本回答不构成项目证据"
          : "本轮未修改项目文件"
    : receipt.outcome === "failed"
      ? "任务失败，不能视为完成"
      : receipt.route === "rescue" ? "只读诊断已结束；未执行修复，下一步仍需确认或授权" : "任务尚未完成或仍需人工确认";
  const undo = appliedEffects.length
    ? "可使用本任务下方的回滚入口撤销已应用批次"
    : rescueRolledBack
      ? "已完成回滚，无需再次撤销"
      : rescueVerified
        ? String(nestedValue(rescueRepair, ["detail", "record", "proposal", "rollbackContract"]) ?? "可使用本任务下方的回滚入口恢复审批前状态")
        : rescueUnverified || rescueRollbackFailed
          ? "先核对当前系统状态，再决定是否执行恢复"
          : receipt.route === "rescue"
            ? "本轮没有系统变更，无需回滚"
            : "没有需要撤销的文件批次";
  return <article className="evidence-receipt" aria-label="结果证据">
    <header><span className="receipt-seal">{rejected ? "×" : receipt.outcome === "completed" ? "✓" : "·"}</span><span><b>任务收据</b><small>{outcome}</small></span></header>
    <dl className="receipt-summary">
      <div><dt>改了什么</dt><dd>{changed}</dd></div>
      <div><dt>本轮请求</dt><dd>{receipt.request}</dd></div>
      <div><dt>验证结果</dt><dd>{verified}</dd></div>
      <div><dt>剩余风险</dt><dd>{risk}</dd></div>
      <div><dt>如何撤销</dt><dd>{undo}</dd></div>
    </dl>
    <details className="receipt-technical"><summary>技术详情 · {total} 条事实</summary><div className="receipt-body"><div className="receipt-counts"><span>来源 <b>{visibleSources.length}</b></span><span>命令 <b>{receipt.commands.length}</b></span><span>测试 <b>{visibleTests.length}</b></span><span>变更 <b>{receipt.effects.length}</b></span><span>决策 <b>{receipt.decisions.length}</b></span></div><p>路由 <code>{receipt.route ?? "未记录"}</code> · SHA-256 <code>{receipt.contentDigest}</code></p></div></details>
    <div className="receipt-actions"><button type="button" onClick={() => void onExport(receipt, "md")}>导出 Markdown</button><button type="button" onClick={() => void onExport(receipt, "json")}>导出 JSON</button></div>
  </article>;
}

function nestedValue(value: unknown, path: string[]): unknown {
  let current = value;
  for (const key of path) {
    if (!current || typeof current !== "object" || Array.isArray(current)) return undefined;
    current = (current as Record<string, unknown>)[key];
  }
  return current;
}

function isTestEvidence(item: Record<string, unknown>) {
  const nested = item.evidence && typeof item.evidence === "object" ? item.evidence as Record<string, unknown> : item;
  const tool = String(nested.toolName ?? nested.tool ?? "");
  return tool === "pytest" || tool.includes("test");
}

function summarizeTestEvidence(tests: Record<string, unknown>[]) {
  const latest = tests.at(-1) ?? {};
  const nested = latest.evidence && typeof latest.evidence === "object" ? latest.evidence as Record<string, unknown> : latest;
  const preview = typeof nested.preview === "string" ? nested.preview : "";
  const paths = Array.isArray(nested.sourcePaths) ? nested.sourcePaths.filter((value): value is string => typeof value === "string") : [];
  const passed = preview.match(/\b(\d+)\s+passed\b/i)?.[1];
  const failed = preview.match(/\b(\d+)\s+failed\b/i)?.[1];
  const exitCode = typeof nested.exitCode === "number" ? nested.exitCode : null;
  const scope = paths.length ? ` · ${paths.join("、")}` : "";
  if (passed && exitCode === 0) return `pytest：${passed} passed${scope}`;
  if (failed || nested.status === "failed" || (exitCode !== null && exitCode !== 0)) return `pytest：${failed ? `${failed} failed` : exitCode !== null ? `退出码 ${exitCode}` : "未通过"}${scope}`;
  if (exitCode === 0 || nested.status === "succeeded") return `pytest 已通过 · ${tests.length} 项真实测试记录${scope}`;
  return `${tests.length} 项测试已有记录`;
}

interface VisibleTaskContext {
  scope: "project" | "system" | "conversation";
  objective: string;
  targetFiles: string[];
  constraints: string[];
  verification: string[];
  unresolvedQuestions: string[];
  revision: number;
}

function latestTaskContext(events: RuntimeEvent[]): VisibleTaskContext | null {
  const payload = [...events].reverse().find((event) => event.eventType === "context.updated")?.payload;
  if (!payload || typeof payload.objective !== "string") return null;
  const strings = (key: string) => Array.isArray(payload[key]) ? (payload[key] as unknown[]).filter((value): value is string => typeof value === "string" && Boolean(value.trim())).slice(0, 10) : [];
  const scope = payload.scope === "system" || payload.scope === "conversation" ? payload.scope : "project";
  return { scope, objective: payload.objective, targetFiles: strings("targetFiles"), constraints: strings("constraints"), verification: strings("verification"), unresolvedQuestions: strings("unresolvedQuestions"), revision: Number(payload.revision ?? 1) };
}

function TaskContextSummary({ context }: { context: VisibleTaskContext }) {
  const heading = context.scope === "project" ? "任务范围" : "任务上下文";
  const scope = context.targetFiles.length
    ? `${context.targetFiles.length} 个目标文件`
    : context.scope === "system"
      ? "系统诊断"
      : context.scope === "conversation"
        ? "普通对话"
        : "尚未限定文件";
  return <details className={`task-context-summary ${context.unresolvedQuestions.length ? "needs-input" : ""}`}>
    <summary><span>{heading}</span><b>{scope}</b><small>v{context.revision}</small></summary>
    <div><p><b>目标</b>{context.objective}</p>{context.targetFiles.length > 0 && <p><b>文件</b>{context.targetFiles.join("、")}</p>}{context.constraints.length > 0 && <p><b>限制</b>{context.constraints.join("；")}</p>}{context.verification.length > 0 && <p><b>验证</b>{context.verification.join("；")}</p>}{context.unresolvedQuestions.length > 0 && <p className="task-context-questions"><b>待确认</b>{context.unresolvedQuestions.join("；")}</p>}</div>
  </details>;
}

export function buildTimeline(ticket: TicketSummary | null, events: RuntimeEvent[], busy: boolean): TimelineItem[] {
  if (!ticket) return [];
  const items: TimelineItem[] = [{ key: `request-${ticket.id}`, kind: "user", content: ticket.request, sequence: 0, attachments: [] }];
  let activity: RuntimeEvent[] = [];
  const flushActivity = (active = false) => {
    if (!activity.length) return;
    items.push({ key: `activity-${activity[0].sequence}`, kind: "activity", events: activity, active });
    activity = [];
  };

  const seenClientRequests = new Set<string>();
  const hiddenStructuredStreams = new Map<string, string>();
  let latestEffectPaths: string[] = [];
  for (const event of [...events].sort((a, b) => a.sequence - b.sequence)) {
    if (event.eventType === "ticket.created") continue;
    if (event.eventType === "user.message") {
      flushActivity();
      const clientRequestId = String(event.payload.clientRequestId ?? "");
      if (clientRequestId && seenClientRequests.has(clientRequestId)) continue;
      if (clientRequestId) seenClientRequests.add(clientRequestId);
      items.push({ key: `user-${event.sequence}`, kind: "user", content: String(event.payload.content ?? ""), sequence: event.sequence, attachments: [] });
    } else if (event.eventType === "context.attached") {
      const user = [...items].reverse().find((item): item is Extract<TimelineItem, { kind: "user" }> => item.kind === "user");
      if (user) user.attachments = attachmentSummaries(event.payload.attachments);
    } else if (event.eventType === "provider.delta") {
      flushActivity();
      const requestId = String(event.payload.requestId ?? "active");
      const key = `stream-${requestId}`;
      const delta = String(event.payload.delta ?? "");
      const existing = items.find((item) => item.kind === "assistant" && item.key === key);
      if (existing?.kind === "assistant") existing.content += delta;
      else {
        const accumulated = `${hiddenStructuredStreams.get(requestId) ?? ""}${delta}`;
        if (looksLikeStructuredStream(accumulated)) hiddenStructuredStreams.set(requestId, accumulated);
        else {
          hiddenStructuredStreams.delete(requestId);
          items.push({ key, kind: "assistant", content: accumulated, streaming: false, sequence: event.sequence, source: "stream" });
        }
      }
    } else if (["provider.message", "delivery.ready"].includes(event.eventType)) {
      flushActivity();
      const content = safeAssistantContent(String(event.payload.content ?? ""));
      if (event.eventType === "delivery.ready") {
        let lastUserIndex = -1;
        for (let index = items.length - 1; index >= 0; index -= 1) {
          if (items[index].kind === "user") { lastUserIndex = index; break; }
        }
        for (let index = items.length - 1; index > lastUserIndex; index -= 1) {
          const item = items[index];
          if (item.kind === "assistant" && (item.source === "stream" || normalizeMessage(item.content) === normalizeMessage(content))) items.splice(index, 1);
        }
      }
      if (content) items.push({ key: `assistant-${event.sequence}`, kind: "assistant", content, streaming: false, sequence: event.sequence, source: event.eventType === "delivery.ready" ? "delivery" : "message" });
      const recovery = taskRescueFrom(event.payload.recovery);
      if (event.eventType === "delivery.ready" && recovery) items.push({ key: `rescue-${event.sequence}`, kind: "rescue", recovery });
    } else if (event.eventType === "effect.proposed") {
      flushActivity();
      const changes = Array.isArray(event.payload.changes) ? event.payload.changes as Array<Record<string, unknown>> : [];
      latestEffectPaths = changes.map((change) => String(change.path ?? "")).filter(Boolean);
      const content = safeAssistantContent(String(event.payload.content ?? ""));
      if (content) items.push({ key: `assistant-${event.sequence}`, kind: "assistant", content, streaming: false, sequence: event.sequence, source: "effect" });
    } else if (event.eventType === "effect.recovery_required") {
      flushActivity();
      items.push({ key: `recovery-${event.sequence}`, kind: "recovery", event, paths: latestEffectPaths });
    } else if (["provider.error", "ticket.failed"].includes(event.eventType)) {
      for (const item of items) {
        if (item.kind !== "assistant" || item.source !== "stream") continue;
        const sanitized = safeAssistantContent(item.content);
        if (sanitized !== item.content) item.content = sanitized;
      }
    } else if (activityCopy[event.eventType]) {
      activity.push(event);
    }
  }
  flushActivity(busy);
  if (busy) {
    const latest = [...items].reverse().find((item): item is Extract<TimelineItem, { kind: "assistant" }> => item.kind === "assistant");
    if (latest?.source === "stream") latest.streaming = true;
  }
  return items;
}

function taskRescueFrom(value: unknown): TaskRescue | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const item = value as Record<string, unknown>;
  if (!["candidate_no_change", "candidate_no_effect"].includes(String(item.reason)) || typeof item.path !== "string") return null;
  const path = item.path.replaceAll("\\", "/").trim();
  if (!path || path.startsWith("/") || /^[A-Za-z]:\//.test(path) || path.split("/").includes("..")) return null;
  const attempts = Number(item.attempts);
  if (!Number.isSafeInteger(attempts) || attempts < 1 || attempts > 5) return null;
  const actions = Array.isArray(item.actions)
    ? item.actions.filter((action): action is TaskRescue["actions"][number] => action === "attach_context" || action === "configure_model")
    : [];
  return { reason: item.reason as TaskRescue["reason"], path, attempts, actions: [...new Set(actions)] };
}

function TaskRescueRow({ recovery, onAttachContext, onConfigureModel }: { recovery: TaskRescue; onAttachContext: () => void; onConfigureModel: () => void }) {
  return <section className="task-rescue" aria-label="任务继续选项">
    <div className="task-rescue-copy"><span aria-hidden="true">↗</span><div><b>帮助本轮继续</b><small><code>{recovery.path}</code> · {recovery.attempts} 次候选无有效实现变化</small></div></div>
    <p>UCode 已在再次写入和运行测试前停止。你可以补充相关实现文件，或明确选择更强的模型端点。</p>
    <div className="task-rescue-actions">
      {recovery.actions.includes("attach_context") && <button type="button" className="task-rescue-primary" onClick={onAttachContext}>补充上下文</button>}
      {recovery.actions.includes("configure_model") && <button type="button" className="quiet" onClick={onConfigureModel}>调整模型</button>}
    </div>
  </section>;
}

interface VisibleRescueRepair { proposal: RescueRepairProposal; record: RescueRepairRecord | null }

interface VisibleRescueProbe { proposal: R1ProbeProposal; record: R1ProbeRecord | null }

function rescueProbeFrom(events: RuntimeEvent[]): VisibleRescueProbe | null {
  const proposed = [...events].reverse().find((event) => event.eventType === "rescue.network_probe.proposed");
  const value = proposed?.payload.proposal;
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const proposal = value as unknown as R1ProbeProposal;
  if (proposal.schemaVersion !== 1 || proposal.risk !== "r1" || !["network.dns.resolve", "network.tcp.probe"].includes(proposal.toolId) || typeof proposal.id !== "string") return null;
  const result = [...events].reverse().find((event) => String(event.payload.proposalId ?? "") === proposal.id && event.eventType.startsWith("rescue.network_probe.") && event.eventType !== "rescue.network_probe.proposed");
  const recordValue = result?.payload.record;
  const record = recordValue && typeof recordValue === "object" && !Array.isArray(recordValue) ? recordValue as unknown as R1ProbeRecord : null;
  return { proposal: record?.proposal ?? proposal, record };
}

function R1NetworkProbeCard({ value, busy, onResolve }: { value: VisibleRescueProbe; busy: boolean; onResolve: (proposalId: string, approved: boolean) => Promise<void> }) {
  const proposal = value.proposal;
  const status = proposal.status;
  const statusCopy = status === "awaiting_approval" ? "等待允许" : status === "observed" ? "探测完成" : status === "rejected" ? "保持离线" : status === "running" ? "探测中" : status === "unavailable" ? "探针不可用" : "已安全停止";
  const toolCopy = proposal.toolId === "network.dns.resolve" ? "DNS 解析" : "TCP 连接";
  return <article className={`rescue-network-probe-card ${status}`} aria-label="Rescue 联网探测">
    <header><span>R1</span><div><b>一次性{toolCopy}</b><small>{statusCopy} · 不会记住本次授权</small></div></header>
    <p>{value.record?.detail ?? proposal.purpose}</p>
    <dl><div><dt>目标</dt><dd><code>{proposal.target}</code></dd></div><div><dt>发送</dt><dd>{proposal.dataBoundary}</dd></div><div><dt>限制</dt><dd>不发送 HTTP 内容，不修改系统，不扫描其他目标</dd></div></dl>
    {value.record?.evidence?.id && <div className="rescue-probe-evidence">证据 <code>{value.record.evidence.id}</code></div>}
    {status === "awaiting_approval" && <div className="approval-actions"><button className="quiet" disabled={busy} onClick={() => void onResolve(proposal.id, false)}>保持离线</button><button className="approve" disabled={busy} onClick={() => void onResolve(proposal.id, true)}>{busy ? "正在探测…" : "允许本次探测"}</button></div>}
  </article>;
}

function rescueRepairFrom(events: RuntimeEvent[]): VisibleRescueRepair | null {
  const proposed = [...events].reverse().find((event) => event.eventType === "rescue.repair.proposed");
  const value = proposed?.payload.proposal;
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const proposal = value as unknown as RescueRepairProposal;
  const knownOperation = (proposal.risk === "r2" && proposal.operationId === "system.user_path.normalize")
    || (proposal.risk === "r2" && proposal.operationId === "network.user_proxy.disable_manual")
    || (proposal.risk === "r3" && proposal.operationId === "system.machine_path.normalize")
    || (proposal.risk === "r3" && proposal.operationId === "system.service.start");
  if (proposal.schemaVersion !== 1 || !knownOperation || typeof proposal.id !== "string") return null;
  const result = [...events].reverse().find((event) => String(event.payload.proposalId ?? "") === proposal.id && event.eventType.startsWith("rescue.repair.") && event.eventType !== "rescue.repair.proposed");
  const recordValue = result?.payload.record;
  const record = recordValue && typeof recordValue === "object" && !Array.isArray(recordValue) ? recordValue as unknown as RescueRepairRecord : null;
  return { proposal: record?.proposal ?? proposal, record };
}

function RescueRepairCard({ value, busy, onResolve, onRollback }: { value: VisibleRescueRepair; busy: boolean; onResolve: (proposalId: string, approved: boolean) => Promise<void>; onRollback: (proposalId: string) => Promise<void> }) {
  const [confirmRollback, setConfirmRollback] = useState(false);
  const proposal = value.proposal;
  const elevated = proposal.risk === "r3";
  const proxyRepair = proposal.operationId === "network.user_proxy.disable_manual";
  const serviceRepair = proposal.operationId === "system.service.start";
  const status = proposal.status;
  const removed = Array.isArray(proposal.normalizedArguments.removedEntries) ? proposal.normalizedArguments.removedEntries : [];
  const statusCopy = status === "awaiting_approval" ? "等待批准" : status === "elevation_requested" ? "等待 Windows UAC" : status === "uac_cancelled" ? "UAC 未批准，未修改" : status === "verified" ? "写入已验证" : status === "rolled_back" ? "已精确回滚" : status === "repaired_unverified" ? "写入状态未完全验证" : status === "rollback_failed" ? "回滚失败" : "已安全停止";
  return <article className={`rescue-repair-card ${status}`} aria-label="Rescue 可逆修复">
    <header><span>{elevated ? "R3" : "R2"}</span><div><b>{serviceRepair ? "启动 Windows Time 服务" : elevated ? "规范化机器 PATH" : proxyRepair ? "禁用当前用户手动代理" : "规范化当前用户 PATH"}</b><small>{statusCopy} · {elevated ? "需要 Windows UAC 管理员确认" : "不需要管理员权限"}</small></div></header>
    <p>{value.record?.detail ?? (serviceRepair ? "真实服务证据显示 W32Time 已停止且固定依赖 RpcSs 正在运行。批准后白名单 helper 只启动 W32Time，不修改启动类型；回滚需要第二次 UAC。" : elevated ? "检测到机器 PATH 中可以确定清理的问题。模型无权提权；批准后只启动一次短生命周期白名单 helper。" : proxyRepair ? "真实代理探针观察到当前用户手动代理已启用。批准后只关闭 ProxyEnable，不删除代理服务器地址、绕过列表或 PAC 配置。" : "检测到可以确定清理的空项、重复项或不存在目录。模型无权执行；只有本地审批执行器能写入。")}</p>
    <dl><div><dt>目标</dt><dd><code>{proposal.target}</code></dd></div><div><dt>验证</dt><dd>{proposal.verificationProbe}</dd></div><div><dt>回滚</dt><dd>{proposal.rollbackContract}</dd></div></dl>
    {removed.length > 0 && <details><summary>查看 {removed.length} 个拟清理项</summary><ul>{removed.slice(0, 12).map((item, index) => <li key={`${item}-${index}`}><code>{item}</code></li>)}</ul></details>}
    <div className="rescue-repair-digests"><span>before <code>{proposal.beforeDigest.slice(0, 10)}</code></span><span>after <code>{proposal.expectedAfterDigest.slice(0, 10)}</code></span></div>
    {status === "awaiting_approval" && <div className="approval-actions"><button className="quiet" disabled={busy} onClick={() => void onResolve(proposal.id, false)}>暂不修复</button><button className="approve" disabled={busy} onClick={() => void onResolve(proposal.id, true)}>{busy ? "正在核验…" : elevated ? "批准并打开 UAC" : "批准、应用并验证"}</button></div>}
    {(status === "verified" || status === "repaired_unverified") && (confirmRollback ? <div className="rollback-confirm"><p>仅当当前目标仍等于应用后摘要时才会恢复；后续用户修改不会被覆盖。{elevated ? "回滚会再次打开 UAC。" : ""}</p><button className="quiet" onClick={() => setConfirmRollback(false)}>取消</button><button className="danger" disabled={busy} onClick={() => void onRollback(proposal.id).finally(() => setConfirmRollback(false))}>{elevated ? "确认并打开 UAC" : "确认回滚"}</button></div> : <button className="quiet rescue-rollback" disabled={busy} onClick={() => setConfirmRollback(true)}>{status === "repaired_unverified" ? "按快照安全回滚" : "回滚本次系统修改"}</button>)}
  </article>;
}

function RecoveryRequiredCard({ event, paths }: { event: RuntimeEvent; paths: string[] }) {
  return <article className="recovery-required-card" role="alert">
    <div className="recovery-required-heading"><span>!</span><div><b>工作区需要人工恢复</b><small>自动写入已停止，此任务不能再次批准或继续修改</small></div></div>
    <p>UCode 无法确认本批次的全部文件已经恢复到原始摘要。为避免覆盖可恢复内容，相关备份会保留在项目目录中。</p>
    {paths.length > 0 && <div className="recovery-required-paths" aria-label="需要检查的文件">{paths.map((item) => <code key={item}>{item}</code>)}</div>}
    <ol><li>先在右侧 Diff 和文件面板检查当前内容。</li><li>在项目目录查找同名的 <code>.ucode-*.bak</code> 文件并人工比对。</li><li>确认恢复完成后新建任务；不要在本任务中重试审批。</li></ol>
    <details><summary>技术详情</summary><p>{String(event.payload.message ?? "文件事务恢复摘要不一致")}</p></details>
  </article>;
}

function normalizeMessage(value: string) {
  return value.trim().replace(/\r\n/g, "\n");
}

function ActivityBlock({ events, active }: { events: RuntimeEvent[]; active: boolean }) {
  const [open, setOpen] = useState(active);
  useEffect(() => { setOpen(active); }, [active]);
  const completed = events.filter((event) => event.eventType === "tool.completed");
  const scoutCount = Number(events.find((event) => event.eventType === "subagents.started")?.payload.count ?? 0);
  const label = scoutCount > 0
    ? `${active ? "并行分析中" : "并行分析完成"} · ${scoutCount} 个本地 Agent`
    : active
    ? "正在处理"
    : completed.length === 1
      ? activityLabel(completed[0])
      : `已完成 ${Math.max(completed.length, events.length)} 项操作`;
  return (
    <section className={`thought-block activity-block ${active ? "active" : ""}`}>
      <button className="thought-heading" onClick={() => setOpen((value) => !value)} aria-expanded={open}>
        <span className="thought-mark">{active ? <span className="thinking-dots"><i /><i /><i /></span> : "✓"}</span>
        <span>{label}</span>
        <span className={`chevron ${open ? "open" : ""}`}>⌄</span>
      </button>
      {open && (
        <ol className="thought-steps">
          {(events.length ? events : [{ sequence: 0, eventType: "assignment.started", payload: {}, ticketId: null, createdAt: "" }]).map((event, index) => (
            <li className={active && index === events.length - 1 ? "current" : "done"} key={`${event.sequence}-${index}`}>
              <span className="step-rail" />
              <span>{activityLabel(event)}</span>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

function activityLabel(event: RuntimeEvent) {
  if (event.eventType === "rescue.probe.completed") {
    const summary = String(event.payload.summary ?? "已采集系统证据");
    const tool = String(event.payload.toolId ?? "系统探针");
    return `${tool} · ${summary}`;
  }
  if (event.eventType === "rescue.diagnosis.ready") {
    return event.payload.outcome === "diagnosed" ? "已由证据定位可能原因" : "证据不足，已安全停止并给出下一探针";
  }
  if (event.eventType === "tool.completed") {
    const tool = String(event.payload.tool ?? "");
    const title = typeof event.payload.title === "string" ? event.payload.title : "";
    const evidence = event.payload.evidence as { path?: unknown; sourcePaths?: unknown[]; contentDigest?: unknown; arguments?: { displayName?: unknown } } | undefined;
    const source = typeof evidence?.path === "string"
      ? evidence.path
      : Array.isArray(evidence?.sourcePaths) && evidence.sourcePaths.length === 1
        ? String(evidence.sourcePaths[0])
        : "";
    if (tool === "read_file" && source) return `已读取 ${source}`;
    if (tool === "attachment_read") {
      const name = String(evidence?.arguments?.displayName ?? "附件");
      const digest = String(evidence?.contentDigest ?? "").slice(0, 8);
      return `已读取 ${name}${digest ? ` · ${digest}` : ""}`;
    }
    if (tool === "list_files") return "已建立项目文件索引";
    if (title) return title;
  }
  return activityCopy[event.eventType] ?? event.eventType;
}

function UserMessage({ content, attachments, copied, onCopy, onEdit, onBranch }: { content: string; attachments: ContextAttachmentSummary[]; copied: boolean; onCopy: () => void; onEdit: () => void; onBranch: () => void }) {
  return <article className="message user-message"><div>{content}</div>{attachments.length > 0 && <div className="message-attachments" aria-label="本轮上下文">{attachments.map((attachment) => <AttachmentChip key={attachment.id} attachment={attachment} />)}</div>}<div className="message-actions user-actions"><button type="button" onClick={onCopy} aria-label="复制消息" title="复制消息"><MessageIcon name={copied ? "check" : "copy"} /></button><button type="button" onClick={onEdit} aria-label="编辑后发送" title="编辑后发送"><MessageIcon name="edit" /></button><button type="button" onClick={onBranch} aria-label="从这里分支" title="从这里分支"><MessageIcon name="branch" /></button></div></article>;
}

function AttachmentChip({ attachment, removable = false, onRemove }: { attachment: ContextAttachmentSummary; removable?: boolean; onRemove?: () => void }) {
  const source = attachment.source === "project" ? "项目" : "外部只读";
  return <span className="attachment-chip" title={attachment.relativePath ?? `${attachment.displayName}（外部只读快照）`}>
    <span className="attachment-file">{attachment.displayName}</span>
    <span className={`attachment-source ${attachment.source === "project" ? "project" : "external"}`}>{source}</span>
    <small>{formatBytes(attachment.includedBytes)}{attachment.truncated ? " · 已截断" : ""}</small>
    {removable && <button type="button" aria-label={`移除 ${attachment.displayName}`} onClick={onRemove}>×</button>}
  </span>;
}

function attachmentSummaries(value: unknown): ContextAttachmentSummary[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    if (!item || typeof item !== "object") return [];
    const attachment = item as Record<string, unknown>;
    const source = attachment.source === "project" ? "project" : attachment.source === "external_readonly" ? "external_readonly" : null;
    if (!source || typeof attachment.id !== "string" || typeof attachment.displayName !== "string") return [];
    return [{
      id: attachment.id,
      displayName: attachment.displayName,
      source,
      relativePath: typeof attachment.relativePath === "string" ? attachment.relativePath : null,
      sizeBytes: Number(attachment.sizeBytes ?? 0),
      includedBytes: Number(attachment.includedBytes ?? 0),
      truncated: Boolean(attachment.truncated),
      sha256: typeof attachment.sha256 === "string" ? attachment.sha256 : "",
    }];
  });
}

function formatBytes(value: number) {
  if (value < 1024) return `${value} B`;
  return value < 1024 * 1024 ? `${(value / 1024).toFixed(value >= 10 * 1024 ? 0 : 1)} KiB` : `${(value / (1024 * 1024)).toFixed(1)} MiB`;
}

function MessageIcon({ name }: { name: "copy" | "check" | "edit" | "branch" }) {
  const paths = {
    copy: <><rect x="8" y="8" width="9" height="9" rx="2" /><path d="M6 14H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h7a2 2 0 0 1 2 2v1" /></>,
    check: <path d="m4 10 4 4 8-9" />,
    edit: <><path d="M4 16l3.5-.8L16 6.7 13.3 4 4.8 12.5 4 16Z" /><path d="m11.8 5.5 2.7 2.7" /></>,
    branch: <><circle cx="5" cy="5" r="2" /><circle cx="15" cy="5" r="2" /><circle cx="15" cy="15" r="2" /><path d="M7 5h3a5 5 0 0 1 5 5v3M5 7v8" /></>,
  };
  return <svg viewBox="0 0 20 20" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">{paths[name]}</svg>;
}

function AssistantMessage({ content, streaming, copied, onCopy, onRetry, onBranch }: { content: string; streaming: boolean; copied: boolean; onCopy: () => void; onRetry: () => void; onBranch: () => void }) {
  return (
    <article className="assistant-message">
      <RichText content={content} streaming={streaming} />
      {!streaming && <div className="message-actions"><button type="button" onClick={onCopy} aria-label="复制回答">{copied ? "✓" : "□"}</button><button type="button" onClick={onRetry} aria-label="重新生成">↻</button><button type="button" onClick={onBranch} aria-label="从回答分支">⑂</button></div>}
    </article>
  );
}

function safeAssistantContent(content: string) {
  if (/["'](?:name|type)["']\s*:\s*["'](?:read_file|search|write_file|write_files|diff|pytest)["']/i.test(content) || /<tool_call>/i.test(content)) {
    return "上一轮本地模型生成了无效工具调用，原始 JSON 已隐藏。请重新发送问题，UCode 会使用新的安全路由处理。";
  }
  return content;
}

function looksLikeStructuredStream(content: string) {
  const value = content.trimStart();
  return value.startsWith("[") || value.startsWith("{") || value.startsWith("<tool_call");
}

