import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AppSettings, AssignmentSummary, BootstrapSnapshot, ContextAttachmentSelection, FilePreview, GitDetails, GitSnapshot, ModelPackInfo, NotificationDiagnostics, NotificationIdentityStatus, OnboardingStepId, ProjectFileEntry, ProposedEffect, ProviderSecretStatus, ProviderTestResult, RaceBundle, RuntimeDiagnosticSnapshot, RuntimeEvent, SkillPreview, SkillSummary, SubagentRequest, TerminalCommandAccepted, TerminalInterruptResult, TerminalOutputEvent, TerminalReadResult, TerminalResult, TerminalSession, TerminalStatusEvent, TicketSummary, WorktreeEntry } from "../domain/types";
import type { CandidateRaceTarget } from "../domain/candidateRace";

const demoSnapshot: BootstrapSnapshot = {
  portableRoot: "E:\\UCode",
  providerStatus: "ready",
  settings: { streamEnabled: true, terminalShell: "powershell.exe", gitUserName: "", gitUserEmail: "", theme: "dark", reduceMotion: false, soundEnabled: true, soundVolume: 55, completionNotificationsEnabled: true, allowNetworkProviders: false, modelProvider: "bundled", bundledModelProfile: "qwen2-5-coder-1-5b-q4-k-m", providerBaseUrl: "http://127.0.0.1:1234/v1", providerModel: "", uiFontSize: 18, codeFontSize: 14, sidebarWidth: 276, inspectorWidth: 404 },
  onboarding: { schemaVersion: 1, welcomeState: "completed", completedSteps: ["project", "conversation", "review_change", "terminal", "git", "agent", "evidence"], skippedSteps: [], dismissedTips: [], updatedAt: new Date().toISOString(), demoProjectId: "project-demo" },
  capsuleHealth: { status: "ready", integrity: "development", rootAvailable: true, dataWritable: true, freeBytes: 8 * 1024 * 1024 * 1024, minimumFreeBytes: 256 * 1024 * 1024, canStartRuntime: true, sideEffectsAllowed: true, checkedAt: new Date().toISOString(), issues: [] },
  lastSequence: 4,
  projects: [
    { id: "project-demo", name: "calculator-app", path: "E:\\UCode\\workspaces\\calculator-app", trusted: true },
    { id: "project-notes", name: "notes-app", path: "E:\\UCode\\workspaces\\notes-app", trusted: true },
  ],
  tickets: [
    {
      id: "ticket-demo",
      projectId: "project-demo",
      title: "修复加法函数",
      request: "修复 calculator.py，并运行测试。",
      state: "waiting",
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
      pinned: false, archivedAt: null, parentTicketId: null, branchEventSequence: null,
    },
    {
      id: "ticket-notes",
      projectId: "project-notes",
      title: "整理发布说明",
      request: "整理 README 中的发布说明。",
      state: "completed",
      createdAt: new Date(Date.now() - 60_000).toISOString(),
      updatedAt: new Date(Date.now() - 60_000).toISOString(),
      pinned: false, archivedAt: null, parentTicketId: null, branchEventSequence: null,
    },
  ],
  receipts: [],
  events: [
    { sequence: 1, ticketId: "ticket-demo", eventType: "ticket.created", payload: {}, createdAt: new Date().toISOString() },
    { sequence: 2, ticketId: "ticket-demo", eventType: "assignment.started", payload: {}, createdAt: new Date().toISOString() },
    { sequence: 3, ticketId: "ticket-demo", eventType: "workspace.ready", payload: { project: "calculator-app" }, createdAt: new Date().toISOString() },
    { sequence: 4, ticketId: "ticket-demo", eventType: "effect.proposed", payload: { id: "effect-demo", path: "calculator.py", before: "return a - b", after: "return a + b", risk: "write", content: "我检查了加法函数，问题在于返回表达式使用了减法。已经准备了最小修改，并会在你批准后运行测试。" }, createdAt: new Date().toISOString() },
  ],
};
const demoSubagents = new Map<string, AssignmentSummary[]>();
const demoTerminals = new Map<string, { chunks: TerminalReadResult["chunks"]; state: TerminalReadResult["state"] }>();
const demoRaces = new Map<string, RaceBundle[]>();

function inTauri(): boolean {
  return typeof window !== "undefined" && Boolean(window.__TAURI_INTERNALS__);
}

export async function bootstrap(): Promise<BootstrapSnapshot> {
  return inTauri() ? invoke<BootstrapSnapshot>("bootstrap") : demoSnapshot;
}

export async function recheckCapsule(): Promise<BootstrapSnapshot> {
  return inTauri() ? invoke<BootstrapSnapshot>("recheck_capsule") : demoSnapshot;
}

const demoRuntimeDiagnostics: RuntimeDiagnosticSnapshot = {
  overall: "ready",
  services: [
    { id: "agent", label: "Agent", status: "ready" },
    { id: "controller", label: "Controller", status: "ready" },
    { id: "coder", label: "Coder", status: "ready" },
    { id: "llama", label: "llama runtime", status: "ready" },
  ],
  runtimeRoot: "E:\\UCode\\runtime-pack",
  owned: true,
  starting: false,
  restartAttempts: 0,
  modelProfile: "qwen2-5-coder-1-5b-q4-k-m",
  checkedAt: new Date().toISOString(),
  lastError: null,
};

export async function getRuntimeDiagnostics(): Promise<RuntimeDiagnosticSnapshot> {
  return inTauri() ? invoke<RuntimeDiagnosticSnapshot>("runtime_diagnostics") : demoRuntimeDiagnostics;
}

export async function recoverLocalRuntime(): Promise<RuntimeDiagnosticSnapshot> {
  return inTauri() ? invoke<RuntimeDiagnosticSnapshot>("recover_local_runtime") : demoRuntimeDiagnostics;
}

export async function markFrontendReady(): Promise<number | null> {
  return inTauri() ? invoke<number>("frontend_ready", { location: window.location.href }) : null;
}

export interface MainWindowActivity {
  focused: boolean;
  visible: boolean;
  minimized: boolean;
}

export async function mainWindowActivity(): Promise<MainWindowActivity> {
  if (!inTauri()) return { focused: document.hasFocus(), visible: document.visibilityState === "visible", minimized: false };
  return invoke<MainWindowActivity>("main_window_activity");
}

export async function syncNativeTheme(theme: AppSettings["theme"]): Promise<void> {
  if (!inTauri()) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().setTheme(theme === "system" ? null : theme);
}

export async function showCompletionNotification(): Promise<void> {
  if (inTauri()) await invoke<void>("show_completion_notification");
}

export async function getNotificationIdentityStatus(): Promise<NotificationIdentityStatus> {
  if (inTauri()) return invoke<NotificationIdentityStatus>("notification_identity_status");
  return { appId: "dev.ucode.portable", displayName: "UCode", health: "ready", registered: true, iconUri: "E:\\UCode\\UCode.png", checkedAt: new Date().toISOString(), lastError: null };
}

export async function getNotificationDiagnostics(): Promise<NotificationDiagnostics> {
  if (inTauri()) return invoke<NotificationDiagnostics>("completion_notification_diagnostics");
  return { identity: await getNotificationIdentityStatus(), telemetry: { attempts: 3, shown: 2, activated: 1, failed: 1, lastAttemptAt: new Date().toISOString(), lastShownAt: new Date().toISOString(), lastActivatedAt: null, lastError: null } };
}

export async function repairNotificationIdentity(): Promise<NotificationIdentityStatus> {
  if (inTauri()) return invoke<NotificationIdentityStatus>("repair_notification_identity");
  return getNotificationIdentityStatus();
}

export async function clearNotificationIdentity(): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("clear_notification_identity");
  return { ...demoSnapshot, settings: { ...demoSnapshot.settings, completionNotificationsEnabled: false } };
}

export async function openExternalUrl(url: string): Promise<void> {
  if (inTauri()) {
    await invoke<void>("open_external_url", { url });
    return;
  }
  window.open(url, "_blank", "noopener,noreferrer");
}

export async function selectProject(path: string): Promise<BootstrapSnapshot> {
  return inTauri() ? invoke<BootstrapSnapshot>("select_project", { path }) : demoSnapshot;
}

export async function chooseProject(): Promise<BootstrapSnapshot | null> {
  if (!inTauri()) return demoSnapshot;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const path = await open({ directory: true, multiple: false, title: "选择 UCode 项目" });
  return typeof path === "string" ? selectProject(path) : null;
}

export async function createOnboardingDemo(): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("create_onboarding_demo");
  return { ...demoSnapshot, onboarding: { ...demoSnapshot.onboarding, welcomeState: "active", completedSteps: ["project"] } };
}

export async function setOnboardingWelcome(welcomeState: "active" | "dismissed"): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("set_onboarding_welcome", { welcomeState });
  return { ...demoSnapshot, onboarding: { ...demoSnapshot.onboarding, welcomeState } };
}

export async function completeOnboardingStep(stepId: OnboardingStepId): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("complete_onboarding_step", { stepId });
  return { ...demoSnapshot, onboarding: { ...demoSnapshot.onboarding, completedSteps: [...new Set([...demoSnapshot.onboarding.completedSteps, stepId])] } };
}

export async function skipOnboardingStep(stepId: OnboardingStepId): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("skip_onboarding_step", { stepId });
  return { ...demoSnapshot, onboarding: { ...demoSnapshot.onboarding, skippedSteps: [...new Set([...demoSnapshot.onboarding.skippedSteps, stepId])] } };
}

export async function dismissOnboardingTip(tipId: string): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("dismiss_onboarding_tip", { tipId });
  return { ...demoSnapshot, onboarding: { ...demoSnapshot.onboarding, dismissedTips: [...new Set([...demoSnapshot.onboarding.dismissedTips, tipId])] } };
}

export async function resetOnboarding(): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("reset_onboarding");
  return { ...demoSnapshot, onboarding: { schemaVersion: 1, welcomeState: "new", completedSteps: [], skippedSteps: [], dismissedTips: [], updatedAt: new Date().toISOString(), demoProjectId: null } };
}

export async function createTicket(projectId: string, request: string): Promise<TicketSummary> {
  if (inTauri()) return invoke<TicketSummary>("create_ticket", { projectId, request });
  return {
    id: crypto.randomUUID(),
    projectId,
    title: request.slice(0, 80),
    request,
    state: "open",
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    pinned: false, archivedAt: null, parentTicketId: null, branchEventSequence: null,
  };
}

export async function renameTicket(ticketId: string, title: string): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("rename_ticket", { ticketId, title });
  return { ...demoSnapshot, tickets: demoSnapshot.tickets.map((ticket) => ticket.id === ticketId ? { ...ticket, title } : ticket) };
}

export async function deleteTicket(ticketId: string): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("delete_ticket", { ticketId });
  return { ...demoSnapshot, tickets: demoSnapshot.tickets.filter((ticket) => ticket.id !== ticketId), events: demoSnapshot.events.filter((event) => event.ticketId !== ticketId) };
}

export async function pinTicket(ticketId: string, pinned: boolean): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("pin_ticket", { ticketId, pinned });
  return { ...demoSnapshot, tickets: demoSnapshot.tickets.map((ticket) => ticket.id === ticketId ? { ...ticket, pinned } : ticket) };
}

export async function archiveTicket(ticketId: string, archived: boolean): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("archive_ticket", { ticketId, archived });
  return { ...demoSnapshot, tickets: demoSnapshot.tickets.map((ticket) => ticket.id === ticketId ? { ...ticket, archivedAt: archived ? new Date().toISOString() : null, pinned: archived ? false : ticket.pinned } : ticket) };
}

export async function branchTicket(ticketId: string, throughSequence: number): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("branch_ticket", { ticketId, throughSequence });
  return demoSnapshot;
}

export async function saveSettings(settings: AppSettings): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("save_settings", { settings });
  return { ...demoSnapshot, settings };
}

export async function listModelPacks(): Promise<ModelPackInfo[]> {
  if (inTauri()) return invoke<ModelPackInfo[]>("list_model_packs");
  return [{ id: "qwen2-5-coder-1-5b-q4-k-m", name: "Qwen2.5-Coder 1.5B Q4_K_M", parameterClass: "1.5B", contextSize: 4096, bytes: 1_117_320_928, minimumRamBytes: 2_147_483_648, recommendedRamBytes: 4_294_967_296, taskSuitability: ["日常对话", "代码解释", "明确的小型单文件修改"], releaseEligible: false, state: "verified", error: null }];
}

export async function getProviderSecretStatus(): Promise<ProviderSecretStatus> {
  return inTauri() ? invoke<ProviderSecretStatus>("provider_secret_status") : { saved: false, storage: "windows-credential-manager" };
}

export async function saveProviderSecret(secret: string): Promise<ProviderSecretStatus> {
  return inTauri() ? invoke<ProviderSecretStatus>("save_provider_secret", { secret }) : { saved: Boolean(secret), storage: "windows-credential-manager" };
}

export async function clearProviderSecret(): Promise<ProviderSecretStatus> {
  return inTauri() ? invoke<ProviderSecretStatus>("clear_provider_secret") : { saved: false, storage: "windows-credential-manager" };
}

export async function testModelProvider(settings: AppSettings, apiKey: string): Promise<ProviderTestResult> {
  if (inTauri()) return invoke<ProviderTestResult>("test_model_provider", { settings, apiKey: apiKey || null });
  return { ok: true, mode: settings.modelProvider, model: settings.modelProvider === "bundled" ? "bundled-coder" : settings.providerModel, baseUrl: settings.modelProvider === "bundled" ? null : settings.providerBaseUrl };
}

export async function chooseSkillDirectory(): Promise<string | null> {
  if (!inTauri()) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const path = await open({ directory: true, multiple: false, title: "选择包含 SKILL.md 的技能目录" });
  return typeof path === "string" ? path : null;
}

export async function listSkills(): Promise<SkillSummary[]> { return inTauri() ? invoke<SkillSummary[]>("list_skills") : []; }
export async function previewSkill(path: string): Promise<SkillPreview> { return invoke<SkillPreview>("preview_skill", { path }); }
export async function installSkill(preview: SkillPreview): Promise<SkillSummary[]> { return invoke<SkillSummary[]>("install_skill", { preview, approved: true }); }
export async function setSkillEnabled(skillId: string, enabled: boolean): Promise<SkillSummary[]> { return invoke<SkillSummary[]>("set_skill_enabled", { skillId, enabled, approved: true }); }
export async function uninstallSkill(skillId: string): Promise<SkillSummary[]> { return invoke<SkillSummary[]>("uninstall_skill", { skillId, approved: true }); }

export async function exportMarkdown(suggestedName: string, content: string): Promise<string | null> {
  if (!inTauri()) return `${suggestedName}.md`;
  const { save } = await import("@tauri-apps/plugin-dialog");
  const path = await save({
    title: "导出 UCode 对话",
    defaultPath: `${suggestedName.replace(/[<>:\"/\\|?*]/g, "-") || "UCode 对话"}.md`,
    filters: [{ name: "Markdown", extensions: ["md"] }],
  });
  if (!path) return null;
  await invoke<void>("export_markdown", { path, content });
  return path;
}

export async function exportEvidenceReceipt(suggestedName: string, content: string, format: "json" | "md"): Promise<string | null> {
  if (!inTauri()) return `${suggestedName}.${format}`;
  const { save } = await import("@tauri-apps/plugin-dialog");
  const safeName = suggestedName.replace(/[<>:\"/\\|?*]/g, "-") || "UCode Evidence Receipt";
  const path = await save({
    title: "导出证据收据",
    defaultPath: `${safeName}.${format}`,
    filters: [{ name: format === "json" ? "JSON" : "Markdown", extensions: [format] }],
  });
  if (!path) return null;
  await invoke<void>("export_markdown", { path, content });
  return path;
}

export async function chooseContextAttachments(projectId: string, selectionId?: string): Promise<ContextAttachmentSelection | null> {
  if (!inTauri()) {
    return {
      selectionId: "selection-demo",
      attachments: [{
        id: "attachment-demo",
        displayName: "calculator.py",
        source: "project",
        relativePath: "calculator.py",
        sizeBytes: 38,
        includedBytes: 38,
        truncated: false,
        sha256: "demo-sha256",
      }],
      errors: [],
    };
  }
  return invoke<ContextAttachmentSelection | null>("choose_context_attachments", { projectId, selectionId });
}

export async function removeContextAttachment(selectionId: string, attachmentId: string): Promise<void> {
  if (inTauri()) await invoke<void>("remove_context_attachment", { selectionId, attachmentId });
}

export async function listProjectFiles(projectId: string): Promise<ProjectFileEntry[]> {
  if (inTauri()) return invoke<ProjectFileEntry[]>("list_project_files", { projectId });
  return [
    { path: "calculator.py", name: "calculator.py", isDirectory: false, depth: 0, size: 38 },
    { path: "test_calculator.py", name: "test_calculator.py", isDirectory: false, depth: 0, size: 74 },
  ];
}

export async function readProjectFile(projectId: string, path: string): Promise<FilePreview> {
  if (inTauri()) return invoke<FilePreview>("read_project_file", { projectId, path });
  return { path, content: path === "calculator.py" ? "def add(a, b):\n    return a - b\n" : "from calculator import add\n\ndef test_add():\n    assert add(2, 3) == 5\n", truncated: false };
}

export async function readGitStatus(projectId: string): Promise<GitSnapshot> {
  if (inTauri()) return invoke<GitSnapshot>("git_status", { projectId });
  return { isRepository: true, branch: "main", clean: false, lines: [" M calculator.py"] };
}

export async function readGitDetails(projectId: string): Promise<GitDetails> {
  if (inTauri()) return invoke<GitDetails>("git_details", { projectId });
  return { ...(await readGitStatus(projectId)), diff: "diff --git a/calculator.py b/calculator.py\n--- a/calculator.py\n+++ b/calculator.py\n- return a - b\n+ return a + b", stagedDiff: "", history: ["abc1234 Initial commit"], branches: ["* main"], changes: [{ status: " M", path: "calculator.py", originalPath: null, staged: false, unstaged: true }] };
}

export async function applyGitOperation(projectId: string, operation: string, paths: string[], message: string, branch: string, approved: boolean): Promise<GitDetails> {
  if (inTauri()) return invoke<GitDetails>("git_operation", { projectId, operation, paths, message, branch, approved });
  return readGitDetails(projectId);
}

export async function listWorktrees(projectId: string): Promise<WorktreeEntry[]> {
  if (inTauri()) return invoke<WorktreeEntry[]>("list_worktrees", { projectId });
  return [{ path: "E:\\UCode\\workspaces\\calculator-app", branch: "main", head: "abc1234567890", isMain: true, locked: false, prunable: false, dirty: true }];
}

export async function createWorktree(projectId: string, name: string, branch: string, base: string, approved: boolean): Promise<WorktreeEntry[]> {
  if (inTauri()) return invoke<WorktreeEntry[]>("create_worktree", { projectId, name, branch, base, approved });
  return [...await listWorktrees(projectId), { path: `E:\\UCode\\worktrees\\${name}`, branch, head: "def9876543210", isMain: false, locked: false, prunable: false, dirty: false }];
}

export async function removeWorktree(projectId: string, path: string, approved: boolean): Promise<WorktreeEntry[]> {
  if (inTauri()) return invoke<WorktreeEntry[]>("remove_worktree", { projectId, path, approved });
  return (await listWorktrees(projectId)).filter((entry) => entry.path !== path);
}

export async function listSubagents(ticketId: string): Promise<AssignmentSummary[]> {
  if (inTauri()) return invoke<AssignmentSummary[]>("list_subagents", { ticketId });
  return demoSubagents.get(ticketId) ?? [];
}

export async function startSubagents(ticketId: string, requests: SubagentRequest[], approved: boolean): Promise<AssignmentSummary[]> {
  if (inTauri()) return invoke<AssignmentSummary[]>("start_subagents", { ticketId, requests, approved });
  const now = new Date().toISOString();
  const rootId = `assignment-root-${ticketId}`;
  const assignments: AssignmentSummary[] = [
    { id: rootId, ticketId, parentId: null, label: "协调子任务", request: "coordinate", capability: "orchestrate", state: requests.some((request) => request.capability === "edit") ? "waiting" : "completed", result: "子任务已汇总", error: null, externalId: null, worktreePath: null, diff: null, mergedAt: null, createdAt: now, updatedAt: now },
    ...requests.map((request, index) => ({ id: `assignment-${index}`, ticketId, parentId: rootId, label: request.label, request: request.request, capability: request.capability, state: request.capability === "edit" ? "waiting" as const : "completed" as const, result: request.capability === "edit" ? "修改已生成，等待审批" : `已完成“${request.label}”的只读分析。`, error: null, externalId: `demo-${index}`, worktreePath: request.capability === "edit" ? `E:\\UCode\\worktrees\\agent-${index}` : null, diff: request.capability === "edit" ? "--- a/app.ts\n+++ b/app.ts\n@@ -1 +1 @@\n-old\n+new" : null, mergedAt: null, createdAt: now, updatedAt: now })),
  ];
  demoSubagents.set(ticketId, assignments);
  return assignments;
}

export async function cancelSubagents(ticketId: string): Promise<AssignmentSummary[]> {
  if (inTauri()) return invoke<AssignmentSummary[]>("cancel_subagents", { ticketId });
  const next = (demoSubagents.get(ticketId) ?? []).map((assignment) => ["queued", "running", "waiting"].includes(assignment.state) ? { ...assignment, state: "cancelled" as const } : assignment);
  demoSubagents.set(ticketId, next);
  return next;
}

export async function resolveSubagentEffect(assignmentId: string, approved: boolean): Promise<AssignmentSummary[]> {
  if (inTauri()) return invoke<AssignmentSummary[]>("resolve_subagent_effect", { assignmentId, approved });
  for (const [ticketId, assignments] of demoSubagents) {
    if (!assignments.some((assignment) => assignment.id === assignmentId)) continue;
    const next = assignments.map((assignment) => assignment.id === assignmentId ? { ...assignment, state: approved ? "completed" as const : "cancelled" as const, result: approved ? "隔离修改已批准，可审查并合并到主工作区" : "修改已拒绝，主工作区未改变" } : assignment.parentId === null ? { ...assignment, state: "completed" as const } : assignment);
    demoSubagents.set(ticketId, next); return next;
  }
  return [];
}

export async function mergeSubagentChange(assignmentId: string): Promise<AssignmentSummary[]> {
  if (inTauri()) return invoke<AssignmentSummary[]>("merge_subagent_change", { assignmentId, approved: true });
  for (const [ticketId, assignments] of demoSubagents) {
    if (!assignments.some((assignment) => assignment.id === assignmentId)) continue;
    const next = assignments.map((assignment) => assignment.id === assignmentId ? { ...assignment, mergedAt: new Date().toISOString(), result: "修改已合并到主工作区，尚未暂存或提交" } : assignment);
    demoSubagents.set(ticketId, next); return next;
  }
  return [];
}

export async function runTerminalCommand(projectId: string, command: string, approved: boolean): Promise<TerminalResult> {
  if (inTauri()) return invoke<TerminalResult>("terminal_command", { projectId, command, approved });
  return { command, stdout: "demo output\n", stderr: "", exitCode: 0 };
}

export async function startTerminalSession(projectId: string, rows = 30, cols = 100): Promise<TerminalSession> {
  if (inTauri()) return invoke<TerminalSession>("terminal_start", { projectId, rows, cols });
  const sessionId = `demo-terminal-${projectId}`;
  demoTerminals.set(sessionId, { chunks: [], state: "ready" });
  return { sessionId, nextSequence: 0, state: "ready" };
}

export async function writeTerminalCommand(projectId: string, sessionId: string, command: string, approved: boolean): Promise<TerminalCommandAccepted> {
  if (inTauri()) return invoke<TerminalCommandAccepted>("terminal_write", { projectId, sessionId, command, approved });
  const terminal = demoTerminals.get(sessionId);
  if (terminal) {
    terminal.chunks.push({ sequence: terminal.chunks.length, data: `PS> ${command}\r\ndemo output\r\nPS E:\\UCode> `, at: new Date().toISOString() });
    terminal.state = "ready";
  }
  return { commandId: `demo-command-${Date.now()}`, state: "running" };
}

export async function readTerminalSession(sessionId: string, afterSequence: number | null, limit = 256): Promise<TerminalReadResult> {
  if (inTauri()) return invoke<TerminalReadResult>("terminal_read", { sessionId, afterSequence, limit });
  const terminal = demoTerminals.get(sessionId) ?? { chunks: [], state: "exited" as const };
  const chunks = terminal.chunks.filter((chunk) => afterSequence === null || chunk.sequence > afterSequence).slice(0, limit);
  return { sessionId, chunks, nextSequence: terminal.chunks.length, truncated: false, state: terminal.state, activeCommandId: null };
}

export async function resizeTerminalSession(sessionId: string, rows: number, cols: number): Promise<void> {
  if (inTauri()) return invoke<void>("terminal_resize", { sessionId, rows, cols });
}

export async function interruptTerminalSession(sessionId: string): Promise<TerminalInterruptResult> {
  if (inTauri()) return invoke<TerminalInterruptResult>("terminal_interrupt", { sessionId });
  const terminal = demoTerminals.get(sessionId); if (terminal) terminal.state = "ready";
  return { commandId: null, state: "ready", signalSent: false };
}

export async function closeTerminalSession(sessionId: string): Promise<void> {
  if (inTauri()) return invoke<void>("terminal_close", { sessionId });
  demoTerminals.delete(sessionId);
}

export async function subscribeTerminal(
  onOutput: (event: TerminalOutputEvent) => void,
  onStatus: (event: TerminalStatusEvent) => void,
): Promise<UnlistenFn> {
  if (!inTauri()) return () => undefined;
  const unlistenOutput = await listen<TerminalOutputEvent>("terminal://output", (event) => onOutput(event.payload));
  const unlistenStatus = await listen<TerminalStatusEvent>("terminal://status", (event) => onStatus(event.payload));
  return () => { unlistenOutput(); unlistenStatus(); };
}

export async function cancelTicket(ticketId: string): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("cancel_ticket", { ticketId });
  return { ...demoSnapshot, tickets: demoSnapshot.tickets.map((ticket) => ticket.id === ticketId ? { ...ticket, state: "stopped" } : ticket) };
}

export async function runTicket(ticketId: string, selectionId?: string, clientRequestId: string = crypto.randomUUID()): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("run_ticket", { ticketId, selectionId: selectionId || null, clientRequestId });
  return demoSnapshot;
}

export async function listTicketRaces(ticketId: string): Promise<RaceBundle[]> {
  return inTauri() ? invoke<RaceBundle[]>("list_ticket_races", { ticketId }) : demoRaces.get(ticketId) ?? [];
}

export async function startCandidateRace(ticketId: string, targets: CandidateRaceTarget[], acceptance: string[]): Promise<RaceBundle> {
  const allowedPaths = targets.map((target) => target.path);
  if (inTauri()) return invoke<RaceBundle>("start_candidate_race", { ticketId, targets, acceptance });
  const now = new Date().toISOString();
  const raceId = crypto.randomUUID();
  const bundle: RaceBundle = {
    race: { schemaVersion: 1, id: raceId, ticketId, projectFingerprint: "demo", objective: "候选验证", state: "generating", targets, allowedPaths, acceptance, verdict: null, createdAt: now, updatedAt: now },
    candidates: ["minimal_change", "compatibility_first"].map((strategy, index) => ({
      id: `${raceId}-${index + 1}`,
      strategy: strategy as "minimal_change" | "compatibility_first",
      state: "generating",
      provider: "bundled",
      model: "qwen2-5-coder-1-5b-q4-k-m",
      diff: null,
      resultDigest: null,
      facts: [],
      metrics: { modifiedFiles: 0, unrelatedFiles: 0, deletedFiles: 0, changedLines: 0 },
    })),
  };
  demoRaces.set(ticketId, [bundle]);
  return bundle;
}

export async function cancelCandidateRace(raceId: string): Promise<RaceBundle> {
  if (inTauri()) return invoke<RaceBundle>("cancel_candidate_race", { raceId });
  for (const [ticketId, races] of demoRaces) {
    const found = races.find((bundle) => bundle.race.id === raceId);
    if (!found) continue;
    const cancelled: RaceBundle = { ...found, race: { ...found.race, state: "cancelled" }, candidates: found.candidates.map((candidate) => ({ ...candidate, state: candidate.state === "passed" || candidate.state === "failed" ? candidate.state : "cancelled" })) };
    demoRaces.set(ticketId, races.map((bundle) => bundle.race.id === raceId ? cancelled : bundle));
    return cancelled;
  }
  throw new Error("候选竞赛不存在");
}

export async function continueTicket(ticketId: string, request: string, selectionId?: string, clientRequestId: string = crypto.randomUUID()): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("continue_ticket", { ticketId, request, selectionId: selectionId || null, clientRequestId });
  const now = new Date().toISOString();
  return {
    ...demoSnapshot,
    events: [
      ...demoSnapshot.events,
      { sequence: 5, ticketId, eventType: "user.message", payload: { content: request, clientRequestId }, createdAt: now },
      { sequence: 6, ticketId, eventType: "delivery.ready", payload: { content: "## 分析结果\n\n问题来自 `add` 函数的返回表达式。建议保持修改最小：\n\n```python\ndef add(a, b):\n    return a + b\n```\n\n- 保留现有函数签名\n- 修改后运行测试\n\n| 检查项 | 状态 |\n| --- | --- |\n| 语法 | 通过 |\n| 行为 | 待测试 |" }, createdAt: now },
    ],
    lastSequence: 6,
  };
}

export async function listEvents(after: number): Promise<RuntimeEvent[]> {
  if (inTauri()) return invoke<RuntimeEvent[]>("list_events", { after });
  return demoSnapshot.events.filter((event) => event.sequence > after);
}

export async function subscribeRuntimeEvents(onEvent: (event: RuntimeEvent) => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => undefined;
  return listen<RuntimeEvent>("runtime://event", (event) => onEvent(event.payload));
}

export async function resolveEffect(ticketId: string, effect: ProposedEffect, approved: boolean, selectedPaths: string[] | null = null): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("resolve_effect", { ticketId, effectId: effect.id, approved, selectedPaths });
  return {
    ...demoSnapshot,
    tickets: demoSnapshot.tickets.map((ticket) => ticket.id === ticketId ? { ...ticket, state: approved ? "completed" : "waiting" } : ticket),
    events: [
      ...demoSnapshot.events,
      { sequence: 5, ticketId, eventType: approved ? "effect.approved" : "effect.rejected", payload: { effectId: effect.id }, createdAt: new Date().toISOString() },
    ],
    lastSequence: 5,
  };
}

export async function rollbackEffect(ticketId: string, effectId: string, approved: boolean): Promise<BootstrapSnapshot> {
  if (inTauri()) return invoke<BootstrapSnapshot>("rollback_effect", { ticketId, effectId, approved });
  return {
    ...demoSnapshot,
    tickets: demoSnapshot.tickets.map((ticket) => ticket.id === ticketId ? { ...ticket, state: "rolled_back" } : ticket),
  };
}
