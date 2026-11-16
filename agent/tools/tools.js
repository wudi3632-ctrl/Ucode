import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { resolveInside } from "../local_source/workspace.js";
import { AppError } from "../shared/errors.js";

const digest = (value) => createHash("sha256").update(value).digest("hex");
const IGNORED_DIRECTORIES = new Set([
  ".git", ".idea", ".mypy_cache", ".pytest_cache", ".ruff_cache", ".venv", ".vscode",
  "__pycache__", "coverage", "dist", "node_modules", "target",
]);

function limited(value, maxBytes) {
  const bytes = Buffer.from(value);
  return bytes.length <= maxBytes ? value : `${bytes.subarray(0, maxBytes).toString()}\n[output truncated]`;
}

function appendOutput(output, chunk, maxBytes) {
  if (output.bytes >= maxBytes) {
    output.truncated = true;
    return;
  }
  const value = Buffer.from(chunk);
  const kept = value.subarray(0, maxBytes - output.bytes);
  output.chunks.push(kept);
  output.bytes += kept.length;
  output.truncated ||= kept.length !== value.length;
}

function outputText(output) {
  const value = Buffer.concat(output.chunks).toString();
  return output.truncated ? `${value}\n[output truncated]` : value;
}

function run(command, args, cwd, timeoutMs, maxBytes) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd,
      shell: false,
      windowsHide: true,
      env: {
        PATH: process.env.PATH || "",
        SYSTEMROOT: process.env.SYSTEMROOT || "",
        TEMP: process.env.TEMP || os.tmpdir(),
        TMP: process.env.TMP || os.tmpdir(),
        PYTHONIOENCODING: "utf-8",
        PYTHONDONTWRITEBYTECODE: "1",
      },
    });
    const stdout = { chunks: [], bytes: 0, truncated: false };
    const stderr = { chunks: [], bytes: 0, truncated: false };
    child.stdout.on("data", (chunk) => appendOutput(stdout, chunk, maxBytes));
    child.stderr.on("data", (chunk) => appendOutput(stderr, chunk, maxBytes));
    child.on("error", reject);
    const timer = setTimeout(() => child.kill(), timeoutMs);
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      resolve({ exitCode: code, signal, stdout: outputText(stdout), stderr: outputText(stderr) });
    });
  });
}

export class ProjectTools {
  constructor({ timeoutMs, maxOutputBytes, pythonExecutable = "python", fileSystem = fs }) {
    this.timeoutMs = timeoutMs;
    this.maxOutputBytes = maxOutputBytes;
    this.pythonExecutable = pythonExecutable;
    this.fileSystem = fileSystem;
  }

  readFile(root, args) {
    const file = resolveInside(root, args.path);
    const content = fs.readFileSync(file, "utf8");
    return { content: limited(content, this.maxOutputBytes), sha256: digest(content) };
  }

  listFiles(root, { maxFiles = 240 } = {}) {
    const files = [];
    const visit = (directory) => {
      if (files.length >= maxFiles) return;
      for (const name of fs.readdirSync(directory)) {
        if (files.length >= maxFiles) break;
        if (IGNORED_DIRECTORIES.has(name)) continue;
        const entry = path.join(directory, name);
        const stat = fs.lstatSync(entry);
        if (stat.isSymbolicLink()) continue;
        if (stat.isDirectory()) visit(entry);
        else if (stat.isFile() && stat.size <= 2 * 1024 * 1024) files.push(path.relative(root, entry).replaceAll("\\", "/"));
      }
    };
    visit(root);
    return files;
  }

  search(root, args) {
    if (typeof args.query !== "string" || !args.query) throw new AppError("invalid_query", "Search query is required");
    const directory = args.path ? resolveInside(root, args.path) : root;
    const matches = [];
    const visit = (entry) => {
      if (matches.length >= 500) return;
      const stat = fs.lstatSync(entry);
      if (stat.isSymbolicLink()) return;
      if (stat.isDirectory()) {
        if (IGNORED_DIRECTORIES.has(path.basename(entry))) return;
        for (const child of fs.readdirSync(entry)) visit(path.join(entry, child));
        return;
      }
      if (!stat.isFile() || stat.size > 2 * 1024 * 1024) return;
      let text;
      try { text = fs.readFileSync(entry, "utf8"); } catch { return; }
      text.split(/\r?\n/).forEach((line, index) => {
        if (line.includes(args.query) && matches.length < 500) matches.push(`${path.relative(root, entry)}:${index + 1}:${line}`);
      });
    };
    visit(directory);
    return { exitCode: matches.length ? 0 : 1, stdout: limited(matches.join("\n"), this.maxOutputBytes), stderr: "" };
  }

  diff(root, args) {
    const file = resolveInside(root, args.path);
    if (typeof args.content !== "string") throw new AppError("invalid_content", "Diff content must be a string");
    const before = fs.readFileSync(file, "utf8").split(/\r?\n/);
    const after = args.content.split(/\r?\n/);
    const lines = [`--- a/${args.path}`, `+++ b/${args.path}`];
    const count = Math.max(before.length, after.length);
    for (let index = 0; index < count; index += 1) {
      if (before[index] === after[index]) continue;
      if (before[index] !== undefined) lines.push(`-${before[index]}`);
      if (after[index] !== undefined) lines.push(`+${after[index]}`);
    }
    return { exitCode: 0, stdout: limited(lines.join("\n"), this.maxOutputBytes), stderr: "" };
  }

  pytest(root, args) {
    const safeArgs = Array.isArray(args.paths) ? args.paths.map((item) => path.relative(root, resolveInside(root, item))) : [];
    return run(this.pythonExecutable, ["-m", "pytest", "-q", ...safeArgs], root, this.timeoutMs, this.maxOutputBytes);
  }

  prepareWrite(root, args) {
    return this.prepareBatch(root, { changes: [{ path: args.path, content: args.content }] }, "write_file");
  }

  prepareBatch(root, args, tool = "write_files") {
    if (!Array.isArray(args?.changes) || args.changes.length < 1 || args.changes.length > 20) {
      throw new AppError("invalid_changes", "A file batch must contain between 1 and 20 changes");
    }
    const seen = new Set();
    const changes = args.changes.map((change) => {
      if (!change || typeof change.path !== "string") throw new AppError("invalid_path", "Each file change requires a project-relative path");
      const file = resolveInside(root, change.path, { existing: false });
      const pathKey = path.relative(root, file).replaceAll("\\", "/").toLowerCase();
      if (seen.has(pathKey)) throw new AppError("duplicate_path", `File batch contains a duplicate path: ${change.path}`);
      seen.add(pathKey);
      const before = fs.existsSync(file) ? fs.readFileSync(file, "utf8") : null;
      const requestedOperation = String(change.operation || "").toLowerCase();
      const deleting = requestedOperation === "delete";
      if (deleting && before === null) throw new AppError("missing_file", `Cannot delete a missing file: ${change.path}`);
      if (!deleting && typeof change.content !== "string") throw new AppError("invalid_content", `Write content must be a string: ${change.path}`);
      if (!deleting) this.validateReplacement(change.path, before, change.content);
      const operation = deleting ? "delete" : before === null ? "create" : "modify";
      return {
        path: change.path.replaceAll("\\", "/"),
        operation,
        before,
        content: deleting ? null : change.content,
        beforeDigest: before === null ? null : digest(before),
        afterDigest: deleting ? null : digest(change.content),
      };
    });
    const first = changes[0];
    return {
      id: `effect-${randomUUID()}`,
      tool,
      status: "pending",
      changes,
      // Keep the single-file projection while older desktop builds may still
      // read this profile. New code treats `changes` as authoritative.
      path: first.path,
      content: first.content ?? "",
      before: first.before,
      beforeDigest: first.beforeDigest,
      afterDigest: first.afterDigest,
      createdAt: new Date().toISOString(),
    };
  }

  validateReplacement(pathname, before, after) {
    if (typeof after !== "string" || !after.trim()) throw new AppError("candidate_empty", "代码模型生成了空文件");
    const extension = path.extname(pathname).toLowerCase();
    if ([".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx"].includes(extension)
        && (/^\s*(?:async\s+)?def\s+[A-Za-z_]\w*\s*\([^)]*\)\s*:/m.test(after) || /^\s*(?:from\s+\S+\s+import|import\s+\S+(?:\s*,\s*\S+)*\s*$)/m.test(after))) {
      throw new AppError("candidate_language_mismatch", "代码模型把 JavaScript/TypeScript 文件生成成了 Python，已安全拒绝");
    }
    if (extension === ".py" && /^\s*(?:export\s+)?(?:async\s+)?function\s+[A-Za-z_$]/m.test(after)) {
      throw new AppError("candidate_language_mismatch", "代码模型把 Python 文件生成成了 JavaScript，已安全拒绝");
    }
    if (typeof before === "string") {
      if (normalizeComparableContent(after) === normalizeComparableContent(before)) {
        throw new AppError("candidate_no_change", "代码模型没有生成实际内容变化");
      }
      if (after.length < Math.max(8, Math.floor(before.length * 0.35))) {
        throw new AppError("candidate_truncated", "代码模型生成的文件内容异常短");
      }
      const definitions = [...before.matchAll(/^(?:async\s+)?(?:def|class)\s+([A-Za-z_][A-Za-z0-9_]*)/gm)].map((match) => match[1]);
      const scriptDefinitions = [...before.matchAll(/^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?(?:function|class)\s+([A-Za-z_$][A-Za-z0-9_$]*)/gm)].map((match) => match[1]);
      for (const name of definitions) {
        const afterMatches = after.match(new RegExp(`^(?:async\\s+)?(?:def|class)\\s+${name}\\b`, "gm")) || [];
        const beforeMatches = before.match(new RegExp(`^(?:async\\s+)?(?:def|class)\\s+${name}\\b`, "gm")) || [];
        if (!afterMatches.length) {
          throw new AppError("candidate_interface_lost", `代码模型删除了既有接口：${name}`);
        }
        if (afterMatches.length > beforeMatches.length) throw new AppError("candidate_interface_duplicated", `代码模型重复生成了既有接口：${name}`);
      }
      for (const name of scriptDefinitions) {
        const expression = `^\\s*(?:export\\s+)?(?:default\\s+)?(?:async\\s+)?(?:function|class)\\s+${name}\\b`;
        const afterMatches = after.match(new RegExp(expression, "gm")) || [];
        const beforeMatches = before.match(new RegExp(expression, "gm")) || [];
        if (!afterMatches.length) {
          throw new AppError("candidate_interface_lost", `代码模型删除了既有接口：${name}`);
        }
        if (afterMatches.length > beforeMatches.length) throw new AppError("candidate_interface_duplicated", `代码模型重复生成了既有接口：${name}`);
      }
    }
    if ([".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx"].includes(extension)) {
      const opens = (after.match(/[({[]/g) || []).length;
      const closes = (after.match(/[)}\]]/g) || []).length;
      if (Math.abs(opens - closes) > 1) throw new AppError("candidate_syntax_error", "代码模型生成的脚本括号不完整");
    }
    if (extension === ".py") {
      const check = spawnSync(this.pythonExecutable, ["-c", "import ast,sys; ast.parse(sys.stdin.read())"], {
        input: after,
        encoding: "utf8",
        windowsHide: true,
        timeout: Math.min(this.timeoutMs, 10_000),
        env: { PATH: process.env.PATH || "", SYSTEMROOT: process.env.SYSTEMROOT || "", PYTHONIOENCODING: "utf-8" },
      });
      if (check.status !== 0) throw new AppError("candidate_syntax_error", "代码模型生成的 Python 文件未通过语法检查");
    }
    return true;
  }

  applyWrite(root, effect) {
    return this.applyBatch(root, effect);
  }

  applyBatch(root, effect, selectedPaths = null) {
    const changes = normalizeEffectChanges(effect);
    const selected = selectedPaths == null ? changes.map((change) => change.path) : selectedPaths;
    if (!Array.isArray(selected) || selected.length < 1) throw new AppError("empty_selection", "Select at least one file to apply", 422);
    const selectedKeys = new Set(selected.map((item) => String(item).replaceAll("\\", "/").toLowerCase()));
    if (selectedKeys.size !== selected.length) throw new AppError("duplicate_selection", "Selected file paths must be unique", 422);
    const picked = changes.filter((change) => selectedKeys.has(change.path.toLowerCase()));
    if (picked.length !== selectedKeys.size) throw new AppError("invalid_selection", "Selected file is not part of this Effect", 422);

    const validated = picked.map((change) => {
      const file = resolveInside(root, change.path, { existing: false });
      const current = fs.existsSync(file) ? fs.readFileSync(file, "utf8") : null;
      if ((current === null ? null : digest(current)) !== change.beforeDigest) {
        throw new AppError("workspace_changed", `File changed after approval was requested: ${change.path}`, 409);
      }
      return { file, change };
    });
    const prepared = stageTransactionEntries(validated, (change) => change.content, this.fileSystem);
    try {
      runFileTransaction(prepared, this.fileSystem);
    } catch (error) {
      if (error.code === "batch_recovery_failed") effect.status = "recovery_required";
      throw error;
    }
    effect.status = "applied";
    effect.appliedAt = new Date().toISOString();
    effect.appliedPaths = picked.map((change) => change.path);
    return {
      paths: effect.appliedPaths,
      changes: picked.map((change) => ({ path: change.path, operation: change.operation, beforeDigest: change.beforeDigest, afterDigest: change.afterDigest })),
    };
  }

  rollback(root, effect) {
    const changes = normalizeEffectChanges(effect);
    const appliedKeys = new Set((effect.appliedPaths || changes.map((change) => change.path)).map((item) => item.toLowerCase()));
    const picked = changes.filter((change) => appliedKeys.has(change.path.toLowerCase()));
    const validated = picked.map((change) => {
      const file = resolveInside(root, change.path, { existing: false });
      const current = fs.existsSync(file) ? fs.readFileSync(file, "utf8") : null;
      if ((current === null ? null : digest(current)) !== change.afterDigest) {
        throw new AppError("workspace_changed", `Refusing to overwrite changes made after the agent write: ${change.path}`, 409);
      }
      return { file, change };
    });
    const prepared = stageTransactionEntries(validated, (change) => change.before, this.fileSystem);
    try {
      runFileTransaction(prepared, this.fileSystem);
    } catch (error) {
      if (error.code === "batch_recovery_failed") effect.status = "recovery_required";
      throw error;
    }
    effect.status = "rolled_back";
    return { paths: picked.map((change) => change.path), status: effect.status };
  }
}

function normalizeComparableContent(value) {
  return String(value)
    .replace(/^\uFEFF/, "")
    .replace(/\r\n?/g, "\n")
    .split("\n")
    .map((line) => line.replace(/[\t ]+$/g, ""))
    .join("\n")
    .replace(/\n+$/g, "");
}

function normalizeEffectChanges(effect) {
  if (Array.isArray(effect?.changes) && effect.changes.length) return effect.changes;
  return [{
    path: effect.path,
    operation: effect.before === null ? "create" : "modify",
    before: effect.before,
    content: effect.content,
    beforeDigest: effect.beforeDigest,
    afterDigest: effect.afterDigest,
  }];
}

function transactionEntry(file, change, nextContent, fileSystem) {
  const nonce = randomUUID();
  const temporary = `${file}.ucode-${nonce}.tmp`;
  const backup = `${file}.ucode-${nonce}.bak`;
  if (nextContent !== null) fileSystem.writeFileSync(temporary, nextContent, { encoding: "utf8", flag: "wx" });
  return { file, temporary, backup, nextContent, hadCurrent: fileSystem.existsSync(file), applied: false, change };
}

function stageTransactionEntries(validated, contentFor, fileSystem) {
  const entries = [];
  try {
    for (const { file, change } of validated) entries.push(transactionEntry(file, change, contentFor(change), fileSystem));
    return entries;
  } catch (error) {
    for (const entry of entries) {
      if (fileSystem.existsSync(entry.temporary)) fileSystem.unlinkSync(entry.temporary);
    }
    throw new AppError("batch_stage_failed", `File batch could not be staged: ${error.message}`, 500);
  }
}

function runFileTransaction(entries, fileSystem) {
  try {
    for (const entry of entries) {
      if (entry.hadCurrent) fileSystem.renameSync(entry.file, entry.backup);
      if (entry.nextContent !== null) fileSystem.renameSync(entry.temporary, entry.file);
      entry.applied = true;
    }
  } catch (error) {
    const recoveryFailures = [];
    for (const entry of [...entries].reverse()) {
      try {
        if (entry.applied && fileSystem.existsSync(entry.file)) fileSystem.unlinkSync(entry.file);
        if (fileSystem.existsSync(entry.backup)) fileSystem.renameSync(entry.backup, entry.file);
        if (fileSystem.existsSync(entry.temporary)) fileSystem.unlinkSync(entry.temporary);
      } catch (recoveryError) {
        recoveryFailures.push(`${entry.change.path}: ${recoveryError.message}`);
      }
    }
    for (const entry of entries) {
      try {
        const current = fileSystem.existsSync(entry.file) ? fileSystem.readFileSync(entry.file, "utf8") : null;
        const currentDigest = current === null ? null : digest(current);
        if (currentDigest !== entry.change.beforeDigest) recoveryFailures.push(`${entry.change.path}: restored digest mismatch`);
      } catch (verificationError) {
        recoveryFailures.push(`${entry.change.path}: ${verificationError.message}`);
      }
    }
    if (recoveryFailures.length) {
      throw new AppError("batch_recovery_failed", `File batch failed and automatic recovery could not be verified: ${[...new Set(recoveryFailures)].join("; ")}`, 500);
    }
    throw new AppError("batch_apply_failed", `File batch was not applied: ${error.message}`, 500);
  }
  for (const entry of entries) {
    if (fileSystem.existsSync(entry.backup)) fileSystem.unlinkSync(entry.backup);
    if (fileSystem.existsSync(entry.temporary)) fileSystem.unlinkSync(entry.temporary);
  }
}
