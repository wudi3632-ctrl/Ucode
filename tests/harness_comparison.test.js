import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { CASES, selectCases } from "../benchmarks/harness-comparison/cases.js";
import { createFixture, removeFixture } from "../benchmarks/harness-comparison/fixtures.js";
import { scoreResult, summarize } from "../benchmarks/harness-comparison/score.js";
import { runComparison } from "../benchmarks/harness-comparison/run.js";

test("public benchmark has diverse non-repeated fixtures", () => {
  assert.ok(CASES.length >= 10);
  assert.ok(new Set(CASES.map((item) => JSON.stringify(item.files))).size >= 8);
  assert.ok(CASES.some((item) => item.kind === "chat"));
  assert.ok(CASES.some((item) => item.kind === "project_read"));
  assert.ok(CASES.some((item) => item.kind === "action" && item.crossFile));
  assert.ok(selectCases("smoke").length >= 5);
});

test("scorer detects unsupported claims and false completion", () => {
  const item = CASES.find((value) => value.id === "project-overview");
  const root = createFixture(item);
  try {
    const score = scoreResult(item, root, { content: "已完成：入口是 ghost.py。", events: [] });
    assert.equal(score.success, false);
    assert.equal(score.falseCompletion, true);
    assert.equal(score.outcome, "unsafe_failure");
  } finally { removeFixture(root); }
});

test("summary enforces the published Team gate", () => {
  const make = (group, success, elapsedMs, falseCompletion = false) => ({
    group, caseId: "cross", crossFile: true, elapsedMs, toolCalls: 0, activatedAgents: 0,
    metrics: { modelCalls: 1 }, score: { success, falseCompletion, workspaceEscape: false },
  });
  const summary = summarize([make("ucode-single", false, 100), make("ucode-team", true, 180)]);
  assert.equal(summary.decision.crossFileGain, 1);
  assert.equal(summary.decision.meetsTeamThreshold, true);
  const tooSlow = summarize([make("ucode-single", false, 100), make("ucode-team", true, 250)]);
  assert.equal(tooSlow.decision.meetsTeamThreshold, false);
});

test("scripted smoke comparison runs all groups in isolated fixtures", async () => {
  const output = fs.mkdtempSync(path.join(os.tmpdir(), "ucode-benchmark-output-"));
  try {
    const run = await runComparison({ live: false, cases: "smoke", output });
    assert.equal(run.results.length, selectCases("smoke").length * 4);
    assert.ok(run.results.every((item) => item.score.workspaceEscape === false));
    assert.ok(run.results.filter((item) => item.group === "ucode-team" && !item.teamEligible).every((item) => item.activatedAgents === 0));
    assert.ok(fs.existsSync(path.join(output, "results.json")));
    assert.ok(fs.existsSync(path.join(output, "summary.json")));
    assert.match(fs.readFileSync(path.join(output, "REPORT.md"), "utf8"), /Scripted mode validates benchmark mechanics only/);
  } finally { fs.rmSync(output, { recursive: true, force: true }); }
});
