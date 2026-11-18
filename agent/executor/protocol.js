import path from "node:path";
import { createHash } from "node:crypto";
import { AppError } from "../shared/errors.js";

export const EXECUTOR_PROTOCOL_V1 = "ucode.executor.v1";
export const EXECUTOR_MODES = new Set(["chat", "project_read", "action", "candidate"]);
export const CANDIDATE_STRATEGIES = new Set(["minimal_change", "compatibility_first"]);
export const EXECUTOR_EVENT_TYPES = new Set([
  "start",
  "text_delta",
  "activity",
  "effect_proposal",
  "report",
  "complete",
  "failure",
  "cancelled",
]);
const CAPABILITIES = new Set(["chat", "read", "search", "test", "propose_write", "terminal", "git", "worktree", "skill", "subagent", "network"]);

export function validateExecutorRequest(input) {
  object(input, "Executor request");
  if (input.version !== EXECUTOR_PROTOCOL_V1) fail("executor_version", "Unsupported Executor protocol version");
  boundedString(input.requestId, "requestId", 200);
  boundedString(input.turnId, "turnId", 200);
  boundedString(input.threadKey, "threadKey", 500);
  if (!EXECUTOR_MODES.has(input.mode)) fail("executor_mode", "Executor mode is invalid");
  boundedArray(input.inputs, "inputs", 1, 32);
  boundedArray(input.history, "history", 0, 24);
  boundedArray(input.attachments, "attachments", 0, 8);
  object(input.workspace, "workspace");
  boundedString(input.workspace.canonicalRoot, "workspace.canonicalRoot", 32_000);
  boundedString(input.workspace.fingerprint, "workspace.fingerprint", 500);
  boundedArray(input.workspace.files, "workspace.files", 0, 20_000);

  if (["action", "candidate"].includes(input.mode)) {
    object(input.assignment, "assignment");
    object(input.mandate, "mandate");
    boundedString(input.assignment.ticketId, "assignment.ticketId", 200);
    boundedString(input.assignment.assignmentId, "assignment.assignmentId", 200);
    boundedString(input.mandate.policyVersion, "mandate.policyVersion", 200);
    boundedString(input.mandate.projectId, "mandate.projectId", 200);
    boundedArray(input.mandate.capabilities, "mandate.capabilities", 1, CAPABILITIES.size);
    if (input.mandate.capabilities.some((capability) => !CAPABILITIES.has(capability))) fail("executor_capability", "Mandate contains an unsupported capability");
    if (input.mandate.threadKey !== input.threadKey || input.mandate.projectFingerprint !== input.workspace.fingerprint) {
      fail("executor_authority_mismatch", "Mandate does not match the Thread or workspace");
    }
    if (input.mode === "candidate") validateCandidateRequest(input.candidate, input.mandate);
    else if (input.candidate != null) fail("executor_candidate_unexpected", "Action requests cannot carry candidate configuration");
  } else if (input.assignment != null || input.mandate != null) {
    fail("executor_authority_unexpected", "Chat and project-read requests cannot carry action authority");
  } else if (input.candidate != null) {
    fail("executor_candidate_unexpected", "Chat and project-read requests cannot carry candidate configuration");
  }

  for (const item of input.inputs) {
    object(item, "input");
    boundedString(item.id, "input.id", 200);
    boundedString(item.kind, "input.kind", 100);
    if (typeof item.text !== "string" || Buffer.byteLength(item.text, "utf8") > 256_000) fail("executor_input", "Input text is invalid or too large");
    if (typeof item.human !== "boolean") fail("executor_input", "Input human flag is required");
  }
  for (const item of input.history) {
    object(item, "history item");
    if (!["user", "assistant"].includes(item.role)) fail("executor_history", "History role is invalid");
    if (typeof item.content !== "string" || Buffer.byteLength(item.content, "utf8") > 64_000) fail("executor_history", "History content is invalid or too large");
  }
  for (const file of input.workspace.files) validateWorkspaceFile(file);

  const hasToken = typeof input.attachmentToken === "string" && input.attachmentToken.length > 0;
  if (hasToken !== (input.attachments.length > 0)) fail("executor_attachment_binding", "Attachment token and payload must be present together");
  for (const attachment of input.attachments) validateAttachment(attachment);
  return input;
}

function validateCandidateRequest(candidate, mandate) {
  object(candidate, "candidate");
  boundedString(candidate.raceId, "candidate.raceId", 200);
  boundedString(candidate.candidateId, "candidate.candidateId", 200);
  if (!CANDIDATE_STRATEGIES.has(candidate.strategy)) fail("executor_candidate_strategy", "Candidate strategy is invalid");
  boundedArray(candidate.allowedPaths, "candidate.allowedPaths", 1, 20);
  boundedArray(candidate.acceptance, "candidate.acceptance", 1, 16);
  for (const value of candidate.allowedPaths) validateRelativePath(value, "candidate allowed path");
  for (const value of candidate.acceptance) boundedString(value, "candidate acceptance", 2_000);
  if (!Number.isSafeInteger(candidate.generationAttempt) || candidate.generationAttempt < 1 || candidate.generationAttempt > 2) {
    fail("executor_candidate_budget", "Candidate generation attempt must be 1 or 2");
  }
  if (candidate.feedback != null) boundedString(candidate.feedback, "candidate.feedback", 8_000);
  if (!mandate.capabilities.includes("propose_write")) {
    fail("executor_candidate_authority", "Candidate generation requires propose_write authority");
  }
}

export function validateExecutorEvent(event, request) {
  object(event, "Executor event");
  if (event.version !== EXECUTOR_PROTOCOL_V1 || event.requestId !== request.requestId || event.turnId !== request.turnId) {
    fail("executor_event_identity", "Executor event identity does not match its request");
  }
  if (!Number.isSafeInteger(event.sequence) || event.sequence < 1) fail("executor_event_sequence", "Executor event sequence is invalid");
  if (!EXECUTOR_EVENT_TYPES.has(event.type)) fail("executor_event_type", "Executor event type is invalid");
  // Chain-of-thought has no protocol representation. Only factual activity is accepted.
  if (Object.hasOwn(event, "thought") || Object.hasOwn(event, "reasoning") || Object.hasOwn(event, "chainOfThought")) {
    fail("executor_private_reasoning", "Private reasoning is not an Executor event");
  }
  return event;
}

function validateWorkspaceFile(file) {
  object(file, "workspace file");
  validateRelativePath(boundedString(file.path, "workspace file path", 1_000), "workspace file path");
  if (!Number.isSafeInteger(file.sizeBytes) || file.sizeBytes < 0) fail("executor_workspace_file", "Workspace file size is invalid");
  if (file.contentDigest != null && !/^[0-9a-f]{64}$/i.test(file.contentDigest)) fail("executor_workspace_file", "Workspace file digest is invalid");
}

function validateRelativePath(input, label) {
  const value = boundedString(input, label, 1_000).replaceAll("\\", "/");
  if (path.isAbsolute(value) || /^[A-Za-z]:\//.test(value) || value.startsWith("/") || value.split("/").includes("..") || value.includes("\0")) {
    fail("executor_workspace_path", `${label} must stay relative to the project`);
  }
  return value;
}

function validateAttachment(attachment) {
  object(attachment, "attachment");
  boundedString(attachment.id, "attachment.id", 200);
  const displayName = boundedString(attachment.displayName, "attachment.displayName", 500);
  if (/[\\/\0\r\n]/.test(displayName)) fail("executor_attachment", "Attachment display name is invalid");
  if (!["project", "external_readonly"].includes(attachment.source)) fail("executor_attachment", "Attachment source is invalid");
  if (attachment.source === "external_readonly" && attachment.relativePath != null) fail("executor_attachment", "External attachments cannot expose a path");
  if (attachment.source === "project") validateWorkspaceFile({ path: attachment.relativePath, sizeBytes: Buffer.byteLength(attachment.content || ""), contentDigest: attachment.contentDigest });
  if (typeof attachment.content !== "string" || Buffer.byteLength(attachment.content, "utf8") > 120_000) fail("executor_attachment", "Attachment content is invalid or too large");
  const digest = createHash("sha256").update(attachment.content).digest("hex");
  if (digest !== attachment.contentDigest) fail("executor_attachment_digest", "Attachment content digest does not match");
}

function object(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) fail("executor_protocol", `${label} must be an object`);
  return value;
}

function boundedString(value, label, maxLength) {
  if (typeof value !== "string" || !value.trim() || value.length > maxLength) fail("executor_protocol", `${label} is invalid`);
  return value;
}

function boundedArray(value, label, minimum, maximum) {
  if (!Array.isArray(value) || value.length < minimum || value.length > maximum) fail("executor_context_limit", `${label} is outside its bounded limit`);
  return value;
}

function fail(code, message) {
  throw new AppError(code, message, 422);
}
