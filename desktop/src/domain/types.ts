export type TicketState =
  | "open"
  | "queued"
  | "running"
  | "waiting"
  | "stopping"
  | "stopped"
  | "completed"
  | "failed"
  | "rolled_back";

export interface ProjectSummary {
  id: string;
  name: string;
  path: string;
  trusted: boolean;
}

export interface TicketSummary {
  id: string;
  projectId: string;
  title: string;
  request: string;
  state: TicketState;
  createdAt: string;
  updatedAt: string;
  pinned: boolean;
  archivedAt: string | null;
  parentTicketId: string | null;
  branchEventSequence: number | null;
}

export interface AppSettings {
  streamEnabled: boolean;
  terminalShell: string;
  gitUserName: string;
  gitUserEmail: string;
  theme: "dark" | "light" | "system";
  reduceMotion: boolean;
  soundEnabled: boolean;
  soundVolume: number;
  completionNotificationsEnabled: boolean;
  allowNetworkProviders: boolean;
  modelProvider: "bundled" | "openai-compatible";
  bundledModelProfile: string;
  providerBaseUrl: string;
  providerModel: string;
  uiFontSize: number;
  codeFontSize: number;
  sidebarWidth: number;
  inspectorWidth: number;
}

export interface ProviderSecretStatus { saved: boolean; storage: string; }
export interface ProviderTestResult { ok: boolean; mode: AppSettings["modelProvider"]; model: string; baseUrl: string | null; }

export interface ModelPackInfo {
  id: string;
  name: string;
  parameterClass: string;
  contextSize: number;
  bytes: number;
  minimumRamBytes: number;
  recommendedRamBytes: number;
  taskSuitability: string[];
  releaseEligible: boolean;
  state: "verified" | "invalid";
  error: string | null;
}

export interface NotificationIdentityStatus {
  appId: string;
  displayName: string;
  health: "ready" | "missing" | "drifted" | "error";
  registered: boolean;
  iconUri: string | null;
  checkedAt: string;
  lastError: string | null;
}

export interface NotificationDiagnostics {
  identity: NotificationIdentityStatus;
  telemetry: {
    attempts: number;
    shown: number;
    activated: number;
    failed: number;
    lastAttemptAt: string | null;
    lastShownAt: string | null;
    lastActivatedAt: string | null;
    lastError: string | null;
  };
}

export interface RuntimeDiagnosticSnapshot {
  overall: "ready" | "starting" | "degraded" | "stopped" | "missing";
  services: Array<{
    id: string;
    label: string;
    status: "stopped" | "starting" | "ready" | "failed";
  }>;
  runtimeRoot: string | null;
  owned: boolean;
  starting: boolean;
  restartAttempts: number;
  modelProfile: string;
  checkedAt: string;
  lastError: string | null;
}

export interface SkillSummary {
  id: string; name: string; version: string; description: string; relativePath: string;
  capabilities: string[]; instructionsDigest: string; compatible: boolean;
  compatibilityReason: string | null; enabled: boolean; installedAt: string;
}

export interface SkillPreview {
  id: string; name: string; version: string; description: string; sourcePath: string;
  capabilities: string[]; compatible: boolean; compatibilityReason: string | null;
  instructionsDigest: string; fileCount: number; totalBytes: number;
}

export interface RuntimeEvent {
  sequence: number;
  ticketId: string | null;
  eventType: string;
  payload: Record<string, unknown>;
  createdAt: string;
}

export type RaceState = "prepared" | "generating" | "validating" | "decided" | "awaiting_review" | "no_winner" | "applied" | "rolled_back" | "rejected" | "stale" | "cancelled";
export type CandidateState = "queued" | "generating" | "proposed" | "validating" | "passed" | "failed" | "rejected" | "cancelled";
export type CandidateStrategy = "minimal_change" | "compatibility_first";
export type ValidationOutcome = "passed" | "failed" | "skipped";
export type HardFailure = "workspace_escape" | "syntax_error" | "interface_lost" | "required_test_failed" | "test_deleted" | "non_replayable" | "empty_change" | "mandate_incomplete" | "operation_mismatch";

export interface ValidationFact {
  checkId: string;
  outcome: ValidationOutcome;
  summary: string;
  required: boolean;
  hardFailure: HardFailure | null;
  command: string | null;
  exitCode: number | null;
  evidenceDigests: string[];
}

export interface ChangeMetrics {
  modifiedFiles: number;
  unrelatedFiles: number;
  deletedFiles: number;
  changedLines: number;
}

export interface CandidateDecision {
  candidateId: string;
  eligible: boolean;
  reasons: string[];
}

export interface RaceVerdict {
  schemaVersion: number;
  kind: "winner" | "no_winner";
  winnerId: string | null;
  rankedCandidateIds: string[];
  decisions: CandidateDecision[];
  summary: string;
  decidedAt: string;
}

export interface CandidateRace {
  schemaVersion: number;
  id: string;
  ticketId: string;
  projectFingerprint: string;
  objective: string;
  state: RaceState;
  targets: Array<{ path: string; operation: "create" | "modify" | "delete" }>;
  allowedPaths: string[];
  acceptance: string[];
  verdict: RaceVerdict | null;
  createdAt: string;
  updatedAt: string;
}

export interface RaceCandidate {
  id: string;
  strategy: CandidateStrategy;
  state: CandidateState;
  provider: string;
  model: string;
  diff: string | null;
  resultDigest: string | null;
  facts: ValidationFact[];
  metrics: ChangeMetrics;
}

export interface RaceBundle {
  race: CandidateRace;
  candidates: RaceCandidate[];
}

export interface EvidenceReceipt {
  ticketId: string;
  request: string;
  route: string | null;
  outcome: "in_progress" | "waiting_input" | "waiting_approval" | "completed" | "failed" | "cancelled" | "rolled_back";
  sources: Record<string, unknown>[];
  commands: Record<string, unknown>[];
  tests: Record<string, unknown>[];
  effects: Record<string, unknown>[];
  decisions: Record<string, unknown>[];
  finalState: Record<string, unknown>;
  contentDigest: string;
  updatedAt: string;
}

export interface AppliedEffect {
  id: string;
  paths: string[];
  changes: EffectChange[];
}

export type RepairStatus = "awaiting_approval" | "applying" | "repaired_unverified" | "verified" | "safe_stop" | "rolled_back" | "rollback_failed";

export interface RescueRepairProposal {
  schemaVersion: number;
  id: string;
  ticketId: string;
  operationId: string;
  operationVersion: number;
  risk: "r2";
  target: string;
  normalizedArguments: { after?: { value?: string }; removedEntries?: string[] };
  evidenceIds: string[];
  prerequisites: string[];
  snapshotPlan: string;
  expectedPostcondition: string;
  verificationProbe: string;
  rollbackContract: string;
  beforeDigest: string;
  expectedAfterDigest: string;
  nonce: string;
  createdAt: string;
  expiresAt: string;
  status: RepairStatus;
  proposalDigest: string;
}

export interface RescueRepairRecord {
  proposal: RescueRepairProposal;
  snapshot: Record<string, unknown> | null;
  detail: string;
  updatedAt: string;
}

export interface CapsuleHealth {
  status: "ready" | "warning" | "blocked";
  integrity: "verified" | "cached" | "development" | "failed" | string;
  rootAvailable: boolean;
  dataWritable: boolean;
  freeBytes: number | null;
  minimumFreeBytes: number;
  canStartRuntime: boolean;
  sideEffectsAllowed: boolean;
  checkedAt: string;
  issues: string[];
}

export type OnboardingStepId = "project" | "conversation" | "review_change" | "terminal" | "git" | "agent" | "evidence";

export interface OnboardingState {
  schemaVersion: number;
  welcomeState: "new" | "active" | "dismissed" | "completed";
  completedSteps: OnboardingStepId[];
  skippedSteps: OnboardingStepId[];
  dismissedTips: string[];
  updatedAt: string;
  demoProjectId: string | null;
}

export interface BootstrapSnapshot {
  portableRoot: string;
  projects: ProjectSummary[];
  tickets: TicketSummary[];
  events: RuntimeEvent[];
  receipts: EvidenceReceipt[];
  lastSequence: number;
  providerStatus: "offline" | "ready" | "unavailable";
  settings: AppSettings;
  onboarding: OnboardingState;
  capsuleHealth: CapsuleHealth;
}

export interface ProposedEffect {
  id: string;
  changes: EffectChange[];
  risk: "write" | "command" | "network";
  atomic?: boolean;
}

export interface EffectChange {
  path: string;
  operation: "create" | "modify" | "delete";
  before: string;
  after: string;
  beforeDigest: string | null;
  afterDigest: string | null;
}

export interface ProjectFileEntry {
  path: string;
  name: string;
  isDirectory: boolean;
  depth: number;
  size: number;
}

export interface FilePreview {
  path: string;
  content: string;
  truncated: boolean;
}

export interface ContextAttachmentSummary {
  id: string;
  displayName: string;
  source: "project" | "external_readonly";
  relativePath: string | null;
  sizeBytes: number;
  includedBytes: number;
  truncated: boolean;
  sha256: string;
}

export interface ContextAttachmentError {
  displayName: string;
  code: string;
  message: string;
}

export interface ContextAttachmentSelection {
  selectionId: string;
  attachments: ContextAttachmentSummary[];
  errors: ContextAttachmentError[];
}

export interface GitSnapshot {
  isRepository: boolean;
  branch: string;
  clean: boolean;
  lines: string[];
}

export interface GitDetails extends GitSnapshot {
  diff: string;
  stagedDiff: string;
  history: string[];
  branches: string[];
  changes: GitChange[];
}

export interface GitChange {
  status: string;
  path: string;
  originalPath: string | null;
  staged: boolean;
  unstaged: boolean;
}

export interface WorktreeEntry {
  path: string;
  branch: string;
  head: string;
  isMain: boolean;
  locked: boolean;
  prunable: boolean;
  dirty: boolean;
}

export interface AssignmentSummary {
  id: string;
  ticketId: string;
  parentId: string | null;
  label: string;
  request: string;
  capability: "read" | "edit" | "orchestrate";
  state: "queued" | "running" | "waiting" | "completed" | "failed" | "cancelled";
  result: string | null;
  error: string | null;
  externalId: string | null;
  worktreePath: string | null;
  diff: string | null;
  mergedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface SubagentRequest {
  label: string;
  request: string;
  capability: "read" | "edit";
}

export interface TerminalResult {
  command: string;
  stdout: string;
  stderr: string;
  exitCode: number;
}

export interface TerminalSession {
  sessionId: string;
  nextSequence: number;
  state: TerminalState;
}

export interface TerminalOutputEvent {
  sessionId: string;
  sequence: number;
  data: string;
  at: string;
}

export interface TerminalStatusEvent {
  sessionId: string;
  status: TerminalState;
  exitCode: number | null;
  commandId: string | null;
}

export type TerminalState = "starting" | "ready" | "running" | "interrupting" | "closed" | "exited" | "failed";

export interface TerminalCommandAccepted {
  commandId: string;
  state: TerminalState;
}

export interface TerminalInterruptResult {
  commandId: string | null;
  state: TerminalState;
  signalSent: boolean;
}

export interface TerminalReadResult {
  sessionId: string;
  chunks: Array<{ sequence: number; data: string; at: string }>;
  nextSequence: number;
  truncated: boolean;
  state: TerminalState;
  activeCommandId: string | null;
}
