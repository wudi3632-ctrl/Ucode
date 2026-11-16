import { AppError } from "../shared/errors.js";

export const TOOL_POLICY = Object.freeze({
  read_file: "automatic",
  search: "automatic",
  diff: "automatic",
  pytest: "automatic",
  write_file: "approval",
  write_files: "approval",
});

export function assessTool(name) {
  const decision = TOOL_POLICY[name];
  if (!decision) throw new AppError("tool_denied", `Tool is not allowed: ${name}`, 403);
  return decision;
}

export function requireApproval(effect) {
  if (effect.status !== "approved") {
    throw new AppError("approval_required", "This write requires explicit approval", 409, { effectId: effect.id });
  }
}
