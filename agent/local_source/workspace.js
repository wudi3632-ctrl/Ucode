import fs from "node:fs";
import path from "node:path";
import { AppError } from "../shared/errors.js";

export function resolveProjectRoot(value) {
  if (typeof value !== "string" || !value.trim()) throw new AppError("invalid_project", "Project path is required");
  const root = fs.realpathSync(value);
  if (!fs.statSync(root).isDirectory()) throw new AppError("invalid_project", "Project path must be a directory");
  return root;
}

export function resolveInside(root, relative, { existing = true } = {}) {
  if (typeof relative !== "string" || !relative || path.isAbsolute(relative)) {
    throw new AppError("invalid_path", "Path must be a non-empty project-relative path");
  }
  const rootReal = fs.realpathSync(root);
  const candidate = path.resolve(rootReal, relative);
  if (!(candidate === rootReal || candidate.startsWith(`${rootReal}${path.sep}`))) {
    throw new AppError("path_escape", "Path escapes the selected project", 403);
  }
  const parent = existing ? candidate : path.dirname(candidate);
  let parentReal;
  try {
    parentReal = fs.realpathSync(parent);
  } catch {
    throw new AppError("invalid_path", "Parent path does not exist");
  }
  const safe = parentReal === rootReal || parentReal.startsWith(`${rootReal}${path.sep}`);
  if (!safe) throw new AppError("path_escape", "Path escapes the selected project", 403);
  if (!existing && fs.existsSync(candidate)) {
    const targetReal = fs.realpathSync(candidate);
    if (!(targetReal === rootReal || targetReal.startsWith(`${rootReal}${path.sep}`))) {
      throw new AppError("path_escape", "Symbolic link escapes the selected project", 403);
    }
  }
  if (existing) {
    const real = fs.realpathSync(candidate);
    if (!(real === rootReal || real.startsWith(`${rootReal}${path.sep}`))) {
      throw new AppError("path_escape", "Symbolic link escapes the selected project", 403);
    }
    return real;
  }
  return candidate;
}
