import { CSSProperties, KeyboardEvent as ReactKeyboardEvent, PointerEvent as ReactPointerEvent, useEffect, useMemo, useRef, useState } from "react";
import { archiveTicket, bootstrap, branchTicket, cancelTicket, chooseProject, clearNotificationIdentity, completeOnboardingStep, continueTicket, createOnboardingDemo, createTicket, deleteTicket, dismissOnboardingTip, exportEvidenceReceipt, exportMarkdown, getRuntimeDiagnostics, listEvents, mainWindowActivity, markFrontendReady, pinTicket, recheckCapsule, recoverLocalRuntime, renameTicket, resetOnboarding, resolveEffect, rollbackEffect, runTicket, saveSettings, selectProject, setOnboardingWelcome, showCompletionNotification, skipOnboardingStep, subscribeRuntimeEvents } from "../bridge/desktop";
import { Conversation } from "../components/Conversation";
import { Inspector } from "../components/Inspector";
import { Sidebar } from "../components/Sidebar";
import { Settings } from "../components/Settings";
import type { AppliedEffect, BootstrapSnapshot, ContextAttachmentSelection, EvidenceReceipt, OnboardingStepId, ProjectSummary, ProposedEffect, RuntimeDiagnosticSnapshot } from "../domain/types";
import { deriveObservedModelPerformance, type ObservedModelPerformance } from "../domain/modelPerformance";
import { audioFeedback, unlockAudioFeedback } from "../domain/audioFeedback";
import { CompletionNotificationController } from "../domain/completionNotifications";
import { codeFontScale, uiFontScale } from "../domain/typography";

const completionNotifications = new CompletionNotificationController(mainWindowActivity, showCompletionNotification);

export function App() {
  const [snapshot, setSnapshot] = useState<BootstrapSnapshot | null>(null);
  const [activeTicketId, setActiveTicketId] = useState<string | null>(null);
  const [activeProjectId, setActiveProjectId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [effect, setEffect] = useState<ProposedEffect | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [inspectorExpanded, setInspectorExpanded] = useState(false);
  const [lastFailedRequest, setLastFailedRequest] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [paneWidths, setPaneWidths] = useState({ sidebar: 276, inspector: 404 });
  const [resizingPane, setResizingPane] = useState(false);
  const [runtimeRecovering, setRuntimeRecovering] = useState(false);
  const [onboardingBusy, setOnboardingBusy] = useState(false);
  const [onboardingOpen, setOnboardingOpen] = useState(false);
  const cancellingRef = useRef(false);
  const submitInFlightRef = useRef(false);

  useEffect(() => {
    bootstrap().then(async (value) => {
      audioFeedback.baseline(value.lastSequence);
      completionNotifications.baseline(value.lastSequence);
      setSnapshot(value);
      const projectId = value.projects[0]?.id ?? null;
      const ticketId = firstTicketForProject(value, projectId)?.id ?? null;
      setActiveProjectId(projectId);
      setActiveTicketId(ticketId);
      setEffect(effectFrom(value, ticketId));
      setPaneWidths({ sidebar: value.settings.sidebarWidth, inspector: value.settings.inspectorWidth });
      await markFrontendReady();
    });
  }, []);

  useEffect(() => {
    const unlock = () => {
      window.removeEventListener("pointerdown", unlock, true);
      window.removeEventListener("keydown", unlock, true);
      void unlockAudioFeedback().catch(() => undefined);
    };
    window.addEventListener("pointerdown", unlock, { capture: true, once: true });
    window.addEventListener("keydown", unlock, { capture: true, once: true });
    return () => {
      window.removeEventListener("pointerdown", unlock, true);
      window.removeEventListener("keydown", unlock, true);
    };
  }, []);

  useEffect(() => {
    if (!snapshot) return;
    audioFeedback.process(snapshot.events, snapshot.settings);
  }, [snapshot?.events, snapshot?.settings.soundEnabled, snapshot?.settings.soundVolume]);

  useEffect(() => {
    if (!snapshot) return;
    completionNotifications.process(snapshot.events, snapshot.settings);
  }, [snapshot?.events, snapshot?.settings.completionNotificationsEnabled]);

  useEffect(() => {
    if (!snapshot) return;
    const root = document.documentElement;
    root.style.setProperty("--ui-font-scale", String(uiFontScale(snapshot.settings.uiFontSize)));
    root.style.setProperty("--code-font-scale", String(codeFontScale(snapshot.settings.codeFontSize)));
  }, [snapshot?.settings.uiFontSize, snapshot?.settings.codeFontSize]);

  useEffect(() => {
    if (!snapshot || !shouldPollProviderStatus(snapshot.providerStatus, busy)) return;
    let disposed = false;
    let attempts = 0;
    let timer: number | undefined;
    const inspect = async () => {
      attempts += 1;
      try {
        const providerStatus = await detectStartingProviderStatus(getRuntimeDiagnostics, bootstrap);
        if (disposed) return;
        setSnapshot((current) => current && current.providerStatus !== providerStatus ? { ...current, providerStatus } : current);
      } catch {
        // One failed diagnostic call must not turn a healthy UI red. The next
        // bounded poll will retry without changing user data or task state.
      }
      if (attempts >= 30 && timer !== undefined) window.clearInterval(timer);
    };
    void inspect();
    timer = window.setInterval(inspect, 4_000);
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearInterval(timer);
    };
  }, [snapshot?.portableRoot, snapshot?.providerStatus, busy]);

  useEffect(() => {
    const settings = snapshot?.settings;
    if (!settings) return;
    document.documentElement.classList.toggle("reduce-motion", settings.reduceMotion);
    const media = typeof window.matchMedia === "function" ? window.matchMedia("(prefers-color-scheme: light)") : null;
    const applyTheme = () => {
      document.documentElement.dataset.themePreference = settings.theme;
      document.documentElement.dataset.theme = resolveTheme(settings.theme, media?.matches ?? false);
    };
    applyTheme();
    if (settings.theme !== "system" || !media) return;
    media.addEventListener("change", applyTheme);
    return () => media.removeEventListener("change", applyTheme);
  }, [snapshot?.settings]);

  const activeTicket = useMemo(
    () => snapshot?.tickets.find((ticket) => ticket.id === activeTicketId) ?? null,
    [snapshot, activeTicketId],
  );
  const activeProject = useMemo(
    () => snapshot?.projects.find((project) => project.id === (activeTicket?.projectId ?? activeProjectId)) ?? snapshot?.projects[0] ?? null,
    [snapshot, activeTicket?.projectId, activeProjectId],
  );
  const activeReceipt = useMemo(
    () => snapshot?.receipts.find((receipt) => receipt.ticketId === activeTicketId) ?? null,
    [snapshot?.receipts, activeTicketId],
  );
  const appliedEffect = useMemo(
    () => appliedEffectFrom(snapshot, activeTicketId),
    [snapshot, activeTicketId],
  );
  const modelPerformance = useMemo(
    () => modelPerformanceFrom(snapshot, activeTicketId),
    [snapshot?.events, activeTicketId],
  );

  useEffect(() => {
    if (effect) setInspectorOpen(true);
  }, [effect?.id]);

  useEffect(() => {
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    subscribeRuntimeEvents((event) => {
      setSnapshot((current) => current ? mergeRuntimeEvents(current, [event]) : current);
    }).then((value) => { if (disposed) value(); else unsubscribe = value; });
    return () => { disposed = true; unsubscribe?.(); };
  }, []);

  useEffect(() => {
    if (!busy || !snapshot) return;
    const timer = window.setInterval(async () => {
      try {
        const incoming = await listEvents(snapshot.lastSequence);
        if (!incoming.length) return;
        setSnapshot((current) => current ? mergeRuntimeEvents(current, incoming) : current);
      } catch {
        // The running command reports actionable errors through the normal task flow.
      }
    }, 1000);
    return () => window.clearInterval(timer);
  }, [busy, snapshot?.lastSequence]);

  async function submit(request: string, selection?: ContextAttachmentSelection | null, suppliedRequestId?: string): Promise<boolean> {
    if (!snapshot || !activeProject || submitInFlightRef.current) return false;
    submitInFlightRef.current = true;
    const clientRequestId = suppliedRequestId ?? crypto.randomUUID();
    setBusy(true);
    cancellingRef.current = false;
    setError(null);
    setLastFailedRequest(null);
    try {
      let next: BootstrapSnapshot;
      let targetTicketId: string;
      if (activeTicket) {
        targetTicketId = activeTicket.id;
        const optimisticEvent = {
          sequence: snapshot.lastSequence + 1,
          ticketId: activeTicket.id,
          eventType: "user.message",
          payload: { content: request, clientRequestId },
          createdAt: new Date().toISOString(),
        };
        setSnapshot({ ...snapshot, events: [...snapshot.events, optimisticEvent], lastSequence: optimisticEvent.sequence });
        next = await continueTicket(activeTicket.id, request, selection?.selectionId, clientRequestId);
      } else {
        const ticket = await createTicket(activeProject.id, request);
        targetTicketId = ticket.id;
        const pending = { ...snapshot, tickets: [ticket, ...snapshot.tickets] };
        setSnapshot(pending);
        setActiveTicketId(ticket.id);
        setEffect(null);
        next = await runTicket(ticket.id, selection?.selectionId, clientRequestId);
      }
      if (!snapshot.onboarding.completedSteps.includes("conversation") && next.events.some((event) => event.ticketId === targetTicketId && event.eventType === "delivery.ready")) {
        try { next = await completeOnboardingStep("conversation"); } catch { /* The final delivery remains authoritative even if onboarding is unavailable. */ }
      }
      setSnapshot(next);
      setEffect(effectFrom(next, targetTicketId));
      return true;
    } catch (cause) {
      if (!cancellingRef.current) {
        setError(friendlyError(cause));
        setLastFailedRequest(request);
      }
      return false;
    } finally {
      setBusy(false);
      submitInFlightRef.current = false;
    }
  }

  async function stopActive() {
    if (!activeTicket) return;
    cancellingRef.current = true;
    setError(null);
    try { setSnapshot(await cancelTicket(activeTicket.id)); }
    catch (cause) { setError(friendlyError(cause)); }
    finally { setBusy(false); }
  }

  async function openProject() {
    const next = await chooseProject();
    if (!next) return;
    const projectId = next.projects[0]?.id ?? null;
    let guided = next;
    if (!snapshot?.onboarding.completedSteps.includes("project")) {
      try { guided = await completeOnboardingStep("project"); } catch { /* Project selection still succeeds without guide progress. */ }
    }
    setSnapshot(guided);
    setActiveProjectId(projectId);
    const ticketId = firstTicketForProject(guided, projectId)?.id ?? null;
    setActiveTicketId(ticketId);
    setEffect(effectFrom(guided, ticketId));
    if (snapshot?.onboarding.welcomeState !== "completed") {
      setOnboardingOpen(true);
      setInspectorOpen(true);
    }
  }

  async function openProjectPath(path: string) {
    const next = await selectProject(path);
    const projectId = next.projects.find((project) => project.path === path)?.id ?? next.projects[0]?.id ?? null;
    setSnapshot(next);
    setActiveProjectId(projectId);
    setActiveTicketId(null);
    setEffect(null);
    setToast("已打开 Worktree，可以开始新的隔离任务");
    window.setTimeout(() => setToast(null), 2600);
  }

  async function switchProject(project: ProjectSummary) {
    setError(null);
    try {
      const next = await selectProject(project.path);
      const selected = next.projects.find((item) => item.path === project.path) ?? project;
      const ticketId = firstTicketForProject(next, selected.id)?.id ?? null;
      setSnapshot(next);
      setActiveProjectId(selected.id);
      setActiveTicketId(ticketId);
      setEffect(effectFrom(next, ticketId));
    } catch (cause) {
      setError(friendlyError(cause));
    }
  }

  async function decideEffect(approved: boolean, selectedPaths: string[] | null = null) {
    if (!activeTicket || !effect) return;
    setBusy(true);
    setError(null);
    try {
      let next = await resolveEffect(activeTicket.id, effect, approved, selectedPaths);
      if (approved && !snapshot?.onboarding.completedSteps.includes("review_change") && next.events.some((event) => event.ticketId === activeTicket.id && event.eventType === "effect.applied")) {
        try { next = await completeOnboardingStep("review_change"); } catch { /* Applied Effect remains successful independently. */ }
      }
      setSnapshot(next);
      setEffect(effectFrom(next, activeTicket.id));
    } catch (cause) {
      setError(friendlyError(cause));
    } finally {
      setBusy(false);
    }
  }

  async function rollbackAppliedEffect() {
    if (!activeTicket || !appliedEffect) return;
    setBusy(true);
    setError(null);
    try {
      setSnapshot(await rollbackEffect(activeTicket.id, appliedEffect.id, true));
      setToast("修改已安全回滚；文件摘要已重新验证");
      window.setTimeout(() => setToast(null), 3200);
    } catch (cause) {
      setError(friendlyError(cause));
    } finally {
      setBusy(false);
    }
  }

  async function exportReceipt(receipt: EvidenceReceipt, format: "json" | "md") {
    const content = format === "json" ? JSON.stringify(receipt, null, 2) : receiptMarkdown(receipt);
    const path = await exportEvidenceReceipt(`${activeTicket?.title ?? "UCode"}-evidence`, content, format);
    if (path) {
      try { setSnapshot(await completeOnboardingStep("evidence")); } catch { /* Export still completed. */ }
      setToast(`证据收据已导出到 ${path}`);
      window.setTimeout(() => setToast(null), 3200);
    }
  }

  async function startOnboardingDemo() {
    if (onboardingBusy) return;
    setOnboardingBusy(true); setError(null);
    try {
      const next = await createOnboardingDemo();
      const projectId = next.onboarding.demoProjectId ?? next.projects[0]?.id ?? null;
      setSnapshot(next); setActiveProjectId(projectId); setActiveTicketId(null); setEffect(null);
      setOnboardingOpen(true); setInspectorOpen(true);
      setToast("示例项目已准备好，可以从第一次提问开始");
      window.setTimeout(() => setToast(null), 2600);
    } catch (cause) { setError(friendlyError(cause)); }
    finally { setOnboardingBusy(false); }
  }

  async function markOnboardingStep(stepId: OnboardingStepId) {
    if (snapshot?.onboarding.completedSteps.includes(stepId)) return;
    try { setSnapshot(await completeOnboardingStep(stepId)); }
    catch (cause) { setToast(friendlyError(cause)); window.setTimeout(() => setToast(null), 2400); }
  }

  async function renameConversation(ticketId: string, title: string) {
    setError(null);
    try {
      setSnapshot(await renameTicket(ticketId, title));
    } catch (cause) {
      setError(friendlyError(cause));
    }
  }

  async function deleteConversation(ticketId: string) {
    setError(null);
    try {
      const next = await deleteTicket(ticketId);
      setSnapshot(next);
      const projectId = activeProjectId ?? activeTicket?.projectId ?? next.projects[0]?.id ?? null;
      const nextId = firstTicketForProject(next, projectId)?.id ?? null;
      setActiveTicketId(nextId);
      setEffect(effectFrom(next, nextId));
    } catch (cause) {
      setError(friendlyError(cause));
    }
  }

  async function updateConversation(action: "pin" | "archive" | "restore", ticketId: string, value = true) {
    const next = action === "pin" ? await pinTicket(ticketId, value) : await archiveTicket(ticketId, action === "archive");
    setSnapshot(next);
    if (action === "archive" && activeTicketId === ticketId) {
      const nextId = firstTicketForProject(next, activeProjectId)?.id ?? null;
      setActiveTicketId(nextId);
      setEffect(effectFrom(next, nextId));
    }
  }

  async function createBranch(ticketId: string, sequence: number) {
    const next = await branchTicket(ticketId, sequence);
    setSnapshot(next);
    const branch = next.tickets.find((ticket) => ticket.parentTicketId === ticketId && ticket.branchEventSequence === sequence);
    const projectId = branch?.projectId ?? activeProjectId;
    const nextId = branch?.id ?? firstTicketForProject(next, projectId)?.id ?? null;
    setActiveProjectId(projectId);
    setActiveTicketId(nextId);
    setEffect(effectFrom(next, nextId));
  }

  async function exportActiveConversation() {
    if (!activeTicket) return;
    setError(null);
    try {
      const path = await exportMarkdown(activeTicket.title, conversationMarkdown(activeTicket, snapshot?.events ?? []));
      if (path) {
        setToast(`已导出到 ${path}`);
        window.setTimeout(() => setToast(null), 3200);
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  useEffect(() => {
    const handleShortcut = (event: globalThis.KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setSidebarOpen(true);
        window.setTimeout(() => document.querySelector<HTMLInputElement>('[aria-label="搜索会话"]')?.focus(), 0);
      } else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "n") {
        event.preventDefault();
        setActiveTicketId(null);
        setEffect(null);
        setError(null);
        window.setTimeout(() => document.querySelector<HTMLTextAreaElement>('[aria-label="任务输入"]')?.focus(), 0);
      } else if ((event.ctrlKey || event.metaKey) && event.key === ",") {
        event.preventDefault();
        setSettingsOpen(true);
      } else if (event.key === "Escape") {
        if (settingsOpen) setSettingsOpen(false);
        else if (window.innerWidth <= 1120 && inspectorOpen) setInspectorOpen(false);
      }
    };
    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
  }, [settingsOpen, inspectorOpen]);

  if (!snapshot) return <div className="boot-screen"><div className="boot-mark">U</div><p>正在打开便携工作区…</p></div>;
  if (snapshot.capsuleHealth.status === "blocked") return <CapsuleRecovery health={snapshot.capsuleHealth} root={snapshot.portableRoot} onRetry={async () => setSnapshot(await recheckCapsule())} />;

  const effectiveInspectorWidth = inspectorExpanded
    ? Math.max(paneWidths.inspector, Math.min(720, Math.round(window.innerWidth * .44)))
    : paneWidths.inspector;
  const shellStyle = {
    "--sidebar-width": `${paneWidths.sidebar}px`,
    "--inspector-width": `${effectiveInspectorWidth}px`,
  } as CSSProperties;
  const persistPaneWidth = async (side: "left" | "right", width: number) => {
    const nextWidths = side === "left" ? { ...paneWidths, sidebar: width } : { ...paneWidths, inspector: width };
    setPaneWidths(nextWidths);
    setResizingPane(false);
    try {
      setSnapshot(await saveSettings({ ...snapshot.settings, sidebarWidth: nextWidths.sidebar, inspectorWidth: nextWidths.inspector }));
    } catch (cause) {
      setError(friendlyError(cause));
    }
  };
  const recoverRuntime = async () => {
    if (runtimeRecovering) return;
    setRuntimeRecovering(true);
    setError(null);
    try {
      await recoverLocalRuntime();
      setSnapshot(await bootstrap());
      setToast("本地模型服务已恢复");
      window.setTimeout(() => setToast(null), 2200);
    } catch (cause) {
      setError(friendlyError(cause));
      try { setSnapshot(await bootstrap()); } catch { /* Keep the recovery error visible. */ }
    } finally {
      setRuntimeRecovering(false);
    }
  };

  return (
    <div style={shellStyle} className={`app-shell ${sidebarOpen ? "sidebar-open" : "sidebar-closed"} ${inspectorOpen ? "inspector-open" : "inspector-closed"} ${resizingPane ? "is-resizing" : ""}`}>
      {snapshot.capsuleHealth.status === "warning" && <div className="capsule-warning" role="status">便携盘空间不足，新的写入、终端命令和 Git 操作已暂停。请至少释放 256 MiB。</div>}
      <Sidebar
        projects={snapshot.projects}
        tickets={snapshot.tickets}
        activeTicketId={activeTicketId}
        activeProjectId={activeProject?.id ?? null}
        portableRoot={snapshot.portableRoot}
        onSelectProject={(project) => void switchProject(project)}
        onSelectTicket={(id) => { const ticket = snapshot.tickets.find((item) => item.id === id); setActiveProjectId(ticket?.projectId ?? activeProjectId); setActiveTicketId(id); setEffect(effectFrom(snapshot, id)); setError(null); }}
        onNewTask={() => { setActiveTicketId(null); setEffect(null); }}
        onOpenProject={openProject}
        onRenameTicket={renameConversation}
        onDeleteTicket={deleteConversation}
        onPinTicket={(id, pinned) => updateConversation("pin", id, pinned)}
        onArchiveTicket={(id) => updateConversation("archive", id)}
        onRestoreTicket={(id) => updateConversation("restore", id)}
        onOpenSettings={() => setSettingsOpen(true)}
        onOpenHelp={() => { setOnboardingOpen(true); setInspectorOpen(true); }}
      />
      {sidebarOpen && <PaneResizeHandle side="left" value={paneWidths.sidebar} min={210} max={420} defaultValue={276} onResize={(value) => { setResizingPane(true); setPaneWidths((current) => ({ ...current, sidebar: value })); }} onCommit={(value) => void persistPaneWidth("left", value)} />}
      <Conversation
        projectId={activeProject?.id ?? null}
        ticket={activeTicket}
        events={snapshot.events}
        effect={effect}
        receipt={activeReceipt}
        appliedEffect={appliedEffect}
        busy={busy}
        onSubmit={submit}
        onCancel={stopActive}
        onRetry={async () => { await submit(lastFailedRequest ?? activeTicket?.request ?? ""); }}
        onExport={exportActiveConversation}
        onBranch={async (sequence) => { if (activeTicket) await createBranch(activeTicket.id, sequence); }}
        onApprove={(selectedPaths) => decideEffect(true, selectedPaths)}
        onReject={() => decideEffect(false, null)}
        onRollback={rollbackAppliedEffect}
        onExportReceipt={exportReceipt}
        hasProject={snapshot.projects.length > 0}
        onOpenProject={openProject}
        onboarding={snapshot.onboarding}
        onboardingBusy={onboardingBusy}
        onCreateDemo={startOnboardingDemo}
        onDismissOnboarding={async () => { setSnapshot(await setOnboardingWelcome("dismissed")); }}
        onDismissOnboardingTip={async (tipId) => { setSnapshot(await dismissOnboardingTip(tipId)); }}
        onOpenSettings={() => setSettingsOpen(true)}
        error={error}
        sidebarOpen={sidebarOpen}
        inspectorOpen={inspectorOpen}
        onToggleSidebar={() => setSidebarOpen((value) => !value)}
        onToggleInspector={() => setInspectorOpen((value) => !value)}
        providerStatus={snapshot.providerStatus}
        modelProvider={snapshot.settings.modelProvider}
        providerModel={snapshot.settings.providerModel}
        modelPerformance={modelPerformance}
        providerRecovering={runtimeRecovering}
        onRecoverProvider={recoverRuntime}
      />
      {inspectorOpen && !inspectorExpanded && <PaneResizeHandle side="right" value={paneWidths.inspector} min={320} max={760} defaultValue={404} onResize={(value) => { setResizingPane(true); setPaneWidths((current) => ({ ...current, inspector: value })); }} onCommit={(value) => void persistPaneWidth("right", value)} />}
      {inspectorOpen && <button className="inspector-backdrop" type="button" aria-label="关闭任务上下文" onClick={() => setInspectorOpen(false)} />}
      <Inspector ticket={activeTicket} project={activeProject} effect={effect} events={snapshot.events} codeFontSize={snapshot.settings.codeFontSize} onExpandedChange={setInspectorExpanded} onOpenProject={openProjectPath} onClose={() => { setInspectorOpen(false); setOnboardingOpen(false); }} onboarding={snapshot.onboarding} openOnboarding={onboardingOpen} onOnboardingStep={markOnboardingStep} onSkipOnboardingStep={async (stepId) => { setSnapshot(await skipOnboardingStep(stepId)); }} onResetOnboarding={async () => { setSnapshot(await resetOnboarding()); setOnboardingOpen(true); }} onDismissOnboardingTip={async (tipId) => { setSnapshot(await dismissOnboardingTip(tipId)); }} onChooseProject={openProject} onCreateDemo={startOnboardingDemo} />
      {settingsOpen && <Settings value={snapshot.settings} onClose={() => setSettingsOpen(false)} onSave={async (value) => { setSnapshot(await saveSettings(value)); setSettingsOpen(false); }} onClearNotificationIdentity={async () => { setSnapshot(await clearNotificationIdentity()); }} />}
      {toast && <div className="app-toast" role="status">{toast}</div>}
    </div>
  );
}

function CapsuleRecovery({ health, root, onRetry }: { health: BootstrapSnapshot["capsuleHealth"]; root: string; onRetry: () => Promise<void> }) {
  const [checking, setChecking] = useState(false);
  return <main className="capsule-recovery">
    <div className="recovery-mark">!</div>
    <p className="recovery-kicker">UCode Portable Recovery</p>
    <h1>便携工作区未通过启动检查</h1>
    <p>为避免从损坏或断开的运行时继续执行，Agent 服务没有启动。对话数据不会被自动修改。</p>
    <section><b>需要处理</b><ul>{health.issues.map((issue) => <li key={issue}>{issue}</li>)}</ul></section>
    <dl><div><dt>便携根目录</dt><dd title={root}>{root}</dd></div><div><dt>完整性</dt><dd>{health.integrity}</dd></div><div><dt>可用空间</dt><dd>{health.freeBytes === null ? "无法读取" : `${Math.floor(health.freeBytes / 1024 / 1024)} MiB`}</dd></div></dl>
    <button type="button" disabled={checking} onClick={async () => { setChecking(true); try { await onRetry(); } finally { setChecking(false); } }}>{checking ? "正在重新校验…" : "重新检查并启动"}</button>
  </main>;
}

function PaneResizeHandle({ side, value, min, max, defaultValue, onResize, onCommit }: { side: "left" | "right"; value: number; min: number; max: number; defaultValue: number; onResize: (value: number) => void; onCommit: (value: number) => void }) {
  const dragRef = useRef<{ x: number; width: number } | null>(null);
  const clamp = (next: number) => Math.max(min, Math.min(max, next));
  const widthFromPointer = (clientX: number) => {
    const drag = dragRef.current;
    if (!drag) return value;
    return clamp(drag.width + (clientX - drag.x) * (side === "left" ? 1 : -1));
  };
  const pointerDown = (event: ReactPointerEvent<HTMLButtonElement>) => {
    dragRef.current = { x: event.clientX, width: value };
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const pointerMove = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (dragRef.current) onResize(widthFromPointer(event.clientX));
  };
  const pointerUp = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (!dragRef.current) return;
    const next = widthFromPointer(event.clientX);
    dragRef.current = null;
    event.currentTarget.releasePointerCapture(event.pointerId);
    onCommit(next);
  };
  const keyDown = (event: ReactKeyboardEvent<HTMLButtonElement>) => {
    if (!["ArrowLeft", "ArrowRight", "Home"].includes(event.key)) return;
    event.preventDefault();
    const direction = side === "left" ? 1 : -1;
    const next = event.key === "Home" ? defaultValue : clamp(value + (event.key === "ArrowRight" ? 8 : -8) * direction);
    onResize(next);
    onCommit(next);
  };
  return <button type="button" className={`pane-resizer ${side}`} role="separator" aria-orientation="vertical" aria-label={side === "left" ? "调整会话侧栏宽度" : "调整任务面板宽度"} aria-valuemin={min} aria-valuemax={max} aria-valuenow={value} onPointerDown={pointerDown} onPointerMove={pointerMove} onPointerUp={pointerUp} onPointerCancel={() => { dragRef.current = null; }} onDoubleClick={() => onCommit(defaultValue)} onKeyDown={keyDown}><span /></button>;
}

export function modelPerformanceFrom(snapshot: Pick<BootstrapSnapshot, "events"> | null, ticketId: string | null): ObservedModelPerformance | null {
  if (!snapshot || !ticketId) return null;
  return deriveObservedModelPerformance(snapshot.events.filter((event) => event.ticketId === ticketId));
}

export function mergeRuntimeEvents(snapshot: BootstrapSnapshot, incoming: BootstrapSnapshot["events"]): BootstrapSnapshot {
  if (!incoming.length) return snapshot;
  const latestSequence = Math.max(snapshot.lastSequence, ...incoming.map((event) => event.sequence));
  const fresh = incoming.filter((event) =>
    !snapshot.events.some((item) => item.sequence === event.sequence)
    && (snapshot.settings.streamEnabled || event.eventType !== "provider.delta"),
  );
  if (!fresh.length) return latestSequence === snapshot.lastSequence ? snapshot : { ...snapshot, lastSequence: latestSequence };
  return {
    ...snapshot,
    events: [...snapshot.events, ...fresh].sort((a, b) => a.sequence - b.sequence),
    lastSequence: latestSequence,
  };
}

export function resolveTheme(theme: BootstrapSnapshot["settings"]["theme"], prefersLight: boolean): "dark" | "light" {
  return theme === "system" ? (prefersLight ? "light" : "dark") : theme;
}

export function providerStatusFromRuntime(diagnostics: RuntimeDiagnosticSnapshot): BootstrapSnapshot["providerStatus"] {
  if (diagnostics.overall === "ready" || runtimeCoreReady(diagnostics)) return "ready";
  if (diagnostics.overall === "starting") return "offline";
  return "unavailable";
}

export function shouldPollProviderStatus(status: BootstrapSnapshot["providerStatus"], busy: boolean): boolean {
  return !busy && status !== "ready";
}

export function runtimeCoreReady(diagnostics: Pick<RuntimeDiagnosticSnapshot, "services">): boolean {
  return ["agent", "coder"].every((id) => diagnostics.services.some((service) => service.id === id && service.status === "ready"));
}

export async function detectStartingProviderStatus(
  readDiagnostics: () => Promise<RuntimeDiagnosticSnapshot>,
  readBootstrap: () => Promise<BootstrapSnapshot>,
): Promise<BootstrapSnapshot["providerStatus"]> {
  const diagnosticStatus = providerStatusFromRuntime(await readDiagnostics());
  if (diagnosticStatus === "ready") return "ready";
  // Bootstrap uses the same Agent health contract that gates real requests.
  // During cold start it is the authoritative tie-breaker for a strict
  // multi-service diagnostic that may have timed out while models page in.
  const confirmed = await readBootstrap();
  return confirmed.providerStatus === "ready" ? "ready" : diagnosticStatus;
}

export function friendlyError(cause: unknown) {
  const message = cause instanceof Error ? cause.message : String(cause);
  if (/legacy agent request failed|127\.0\.0\.1:3082|localhost:3082|connection refused|actively refused|error sending request/i.test(message)) {
    return "UCode 暂时无法连接本地 Agent。服务可能仍在启动或已经退出，请稍后点击“重试本轮”。";
  }
  if (/timeout|timed out/i.test(message)) return "模型响应超时。已保留收到的内容，可以直接重试本轮。";
  const legacyDetail = message.match(/^legacy agent returned an invalid response:\s*(.+)$/is)?.[1]?.trim();
  if (legacyDetail) return legacyDetail;
  return message;
}

function conversationMarkdown(ticket: NonNullable<BootstrapSnapshot["tickets"][number]>, events: BootstrapSnapshot["events"]) {
  const rows = [`# ${ticket.title}`, "", `- 创建时间：${ticket.createdAt}`, `- 状态：${ticket.state}`, "", "## 用户", "", ticket.request];
  let partial = "";
  const flushPartial = () => {
    if (!partial.trim()) return;
    rows.push("", "## UCode（未完成回答）", "", partial);
    partial = "";
  };
  for (const event of events.filter((item) => item.ticketId === ticket.id).sort((a, b) => a.sequence - b.sequence)) {
    if (event.eventType === "user.message") {
      flushPartial();
      rows.push("", "## 用户", "", String(event.payload.content ?? ""));
    } else if (event.eventType === "provider.delta") {
      partial += String(event.payload.delta ?? "");
    } else if (["delivery.ready", "provider.message"].includes(event.eventType)) {
      partial = "";
      const content = String(event.payload.content ?? "");
      if (content.trim()) rows.push("", "## UCode", "", content);
    }
  }
  flushPartial();
  rows.push("");
  return rows.join("\n");
}

export function effectFrom(snapshot: BootstrapSnapshot, ticketId: string | null): ProposedEffect | null {
  if (!ticketId) return null;
  const events = snapshot.events.filter((event) => event.ticketId === ticketId).sort((a, b) => b.sequence - a.sequence);
  const proposed = events.find((event) => event.eventType === "effect.proposed");
  if (!proposed) return null;
  const resolved = events.some((event) => event.sequence > proposed.sequence && ["effect.approved", "effect.rejected", "effect.recovery_required"].includes(event.eventType));
  if (resolved) return null;
  const payload = proposed.payload;
  const rawChanges = Array.isArray(payload.changes) ? payload.changes : [{
    path: payload.path,
    operation: typeof payload.beforeDigest === "string" ? "modify" : "create",
    before: payload.before,
    after: payload.after,
    beforeDigest: payload.beforeDigest,
    afterDigest: payload.afterDigest,
  }];
  const changes = rawChanges.map((value) => {
    const change = value as Record<string, unknown>;
    const operation: "create" | "modify" | "delete" = change.operation === "delete" ? "delete" : change.operation === "create" ? "create" : "modify";
    return {
      path: String(change.path ?? ""),
      operation,
      before: String(change.before ?? ""),
      after: operation === "delete" ? "" : String(change.after ?? change.content ?? ""),
      beforeDigest: typeof change.beforeDigest === "string" ? change.beforeDigest : null,
      afterDigest: typeof change.afterDigest === "string" ? change.afterDigest : null,
    };
  }).filter((change) => change.path);
  return {
    id: String(payload.id ?? ""), changes,
    risk: (payload.risk === "command" || payload.risk === "network") ? payload.risk : "write",
  };
}

function appliedEffectFrom(snapshot: BootstrapSnapshot | null, ticketId: string | null): AppliedEffect | null {
  if (!snapshot || !ticketId) return null;
  const events = snapshot.events.filter((event) => event.ticketId === ticketId).sort((a, b) => a.sequence - b.sequence);
  const proposals = new Map<string, ProposedEffect>();
  const applied = new Map<string, string[]>();
  const rolledBack = new Set<string>();
  for (const event of events) {
    const effectId = String(event.payload.effectId ?? event.payload.id ?? "");
    if (event.eventType === "effect.proposed" && effectId) {
      const parsed = effectFrom({ ...snapshot, events: [event] }, ticketId);
      if (parsed) proposals.set(effectId, parsed);
    } else if (event.eventType === "effect.applied" && effectId) {
      const result = event.payload.result as Record<string, unknown> | undefined;
      const paths = Array.isArray(result?.paths) ? result.paths.map(String) : [];
      applied.set(effectId, paths);
    }
    else if (event.eventType === "effect.rolled_back" && effectId) rolledBack.add(effectId);
  }
  const proposal = [...proposals.values()].reverse().find((item) => applied.has(item.id) && !rolledBack.has(item.id));
  if (!proposal) return null;
  const appliedPaths = applied.get(proposal.id) ?? [];
  const changes = appliedPaths.length ? proposal.changes.filter((change) => appliedPaths.includes(change.path)) : proposal.changes;
  return { id: proposal.id, paths: changes.map((change) => change.path), changes };
}

function receiptMarkdown(receipt: EvidenceReceipt) {
  const rows = [
    "# UCode Evidence Receipt", "",
    `- Ticket: \`${receipt.ticketId}\``,
    `- Outcome: \`${receipt.outcome}\``,
    `- Route: \`${receipt.route ?? "unknown"}\``,
    `- Receipt SHA-256: \`${receipt.contentDigest}\``,
    `- Updated: ${receipt.updatedAt}`, "",
    "## Request", "", receipt.request, "",
  ];
  for (const [title, values] of [["Sources", receipt.sources], ["Commands", receipt.commands], ["Tests", receipt.tests], ["Effects", receipt.effects], ["Decisions", receipt.decisions]] as const) {
    rows.push(`## ${title}`, "", "```json", JSON.stringify(values, null, 2), "```", "");
  }
  rows.push("## Final state", "", "```json", JSON.stringify(receipt.finalState, null, 2), "```", "");
  return rows.join("\n");
}

function firstTicketForProject(snapshot: BootstrapSnapshot, projectId: string | null) {
  if (!projectId) return null;
  return snapshot.tickets.find((ticket) => ticket.projectId === projectId && !ticket.archivedAt) ?? null;
}
