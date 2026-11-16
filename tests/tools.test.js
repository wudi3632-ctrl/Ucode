import test from "node:test";
import assert from "node:assert/strict";
import { ProjectTools } from "../agent/tools/tools.js";

const tools = new ProjectTools({ timeoutMs: 1_000, maxOutputBytes: 100_000 });

test("replacement validation rejects cross-language JavaScript candidates", () => {
  const before = "export function add(a, b) {\n  return a - b;\n}\n";
  assert.throws(() => tools.validateReplacement("calculator.js", before, "def add(a, b):\n    return a + b\n"), /Python/);
  assert.equal(tools.validateReplacement("calculator.js", before, "export function add(a, b) {\n  return a + b;\n}\n"), true);
  assert.throws(() => tools.validateReplacement("calculator.js", before, "export const value = 1;\n"), /既有接口/);
});

test("replacement validation rejects duplicated existing interfaces", () => {
  const before = "def solve():\n    return 1\n";
  const duplicated = `${before}\nold\n\n${before}\nnew\n`;
  assert.throws(() => tools.validateReplacement("solver.py", before, duplicated), /重复生成了既有接口/);
});

test("replacement validation rejects exact and normalization-only no-op candidates", () => {
  const before = "def solve():\r\n    return 1\r\n";
  assert.throws(
    () => tools.validateReplacement("solver.py", before, before),
    (error) => error.code === "candidate_no_change",
  );
  assert.throws(
    () => tools.validateReplacement("solver.py", before, "\uFEFFdef solve():  \n    return 1\t\n\n"),
    (error) => error.code === "candidate_no_change",
  );
});
