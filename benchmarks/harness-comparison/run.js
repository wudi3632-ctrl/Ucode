import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { CASES, selectCases } from "./cases.js";
import { createFixture, removeFixture } from "./fixtures.js";
import { createLiveProvider, createScriptedProvider, withCallBudget } from "./providers.js";
import { runBare } from "./runners/bare.js";
import { runReact } from "./runners/react.js";
import { runUCode } from "./runners/ucode.js";
import { renderReport } from "./report.js";
import { scoreResult, summarize } from "./score.js";

const ALL_GROUPS = ["bare", "react", "ucode-single", "ucode-team"];

function gitCommit(cwd) {
  try { return execFileSync("git", ["rev-parse", "HEAD"], { cwd, encoding: "utf8", windowsHide: true }).trim(); }
  catch { return "unknown"; }
}

function parseArguments(argv) {
  const value = { live: false, coderUrl: "http://127.0.0.1:3284", groups: ALL_GROUPS, cases: "smoke", output: null };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--live") value.live = true;
    else if (argument === "--dry-run") value.live = false;
    else if (argument === "--coder-url") value.coderUrl = argv[++index];
    else if (argument === "--groups") value.groups = argv[++index].split(",").map((item) => item.trim()).filter(Boolean);
    else if (argument === "--cases") value.cases = argv[++index];
    else if (argument === "--output") value.output = argv[++index];
    else throw new Error(`Unknown argument: ${argument}`);
  }
  for (const group of value.groups) if (!ALL_GROUPS.includes(group)) throw new Error(`Unknown group: ${group}`);
  return value;
}

async function runGroup(group, context) {
  if (group === "bare") return runBare(context);
  if (group === "react") return runReact(context);
  if (group === "ucode-single") return runUCode({ ...context, team: false });
  return runUCode({ ...context, team: true });
}

export async function runComparison({ live = false, coderUrl = "http://127.0.0.1:3284", groups = ALL_GROUPS, cases = "smoke", output = null } = {}) {
  const selectedCases = selectCases(cases);
  if (!selectedCases.length) throw new Error(`No benchmark cases matched: ${cases}`);
  const provider = live ? createLiveProvider(coderUrl) : createScriptedProvider(new Map(CASES.map((item) => [item.id, item])));
  const health = await provider.health();
  const results = [];
  for (const item of selectedCases) {
    for (const group of groups) {
      const root = createFixture(item);
      const callIndex = provider.calls.length;
      const started = performance.now();
      let execution;
      try {
        execution = await runGroup(group, { item, root, provider: withCallBudget(provider, 6) });
      } catch (error) {
        execution = { content: `安全停止：${error.message}`, writtenPaths: [], toolCalls: 0, activatedAgents: 0, events: [{ type: "error", code: error.code || "error", message: error.message }] };
      }
      const elapsedMs = performance.now() - started;
      const score = scoreResult(item, root, execution);
      results.push({
        caseId: item.id,
        suite: item.suite,
        crossFile: item.crossFile === true,
        teamEligible: item.teamEligible === true,
        group,
        content: execution.content.slice(0, 4_000),
        writtenPaths: execution.writtenPaths,
        toolCalls: execution.toolCalls,
        activatedAgents: execution.activatedAgents,
        elapsedMs,
        metrics: provider.metricsSince(callIndex),
        score,
      });
      removeFixture(root);
    }
  }
  const summary = summarize(results);
  const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const metadata = {
    mode: live ? "live" : "scripted-ci",
    coderUrl: live ? coderUrl : null,
    health,
    gitCommit: gitCommit(repositoryRoot),
    createdAt: new Date().toISOString(),
    caseIds: selectedCases.map((item) => item.id),
    groups,
  };
  const run = { metadata, results, summary };
  if (output) {
    const outputDirectory = path.resolve(output);
    fs.mkdirSync(outputDirectory, { recursive: true });
    fs.writeFileSync(path.join(outputDirectory, "results.json"), `${JSON.stringify(results, null, 2)}\n`);
    fs.writeFileSync(path.join(outputDirectory, "summary.json"), `${JSON.stringify({ metadata, ...summary }, null, 2)}\n`);
    fs.writeFileSync(path.join(outputDirectory, "REPORT.md"), renderReport(run));
  }
  return run;
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  if (!options.output) options.output = path.join(os.tmpdir(), `ucode-harness-${Date.now()}`);
  const run = await runComparison(options);
  process.stdout.write(`${renderReport(run)}\n\nArtifacts: ${path.resolve(options.output)}\n`);
  if (run.results.some((item) => item.score.workspaceEscape)) process.exitCode = 2;
}

if (path.resolve(process.argv[1] || "") === fileURLToPath(import.meta.url)) main().catch((error) => { console.error(error); process.exitCode = 1; });
