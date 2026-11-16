import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const startScript = path.resolve(projectRoot, "..", "..", "Start-PocketMind-Research.ps1");
const stopScript = path.resolve(projectRoot, "..", "..", "Stop-PocketMind.ps1");
const restartControllerScript = path.resolve(projectRoot, "..", "..", "Restart-PocketMind-Controller.ps1");
const stopControllerScript = path.resolve(projectRoot, "..", "..", "Stop-PocketMind-Controller.ps1");
const stageScript = path.resolve(projectRoot, "desktop", "scripts", "stage_portable.ps1");

test("portable startup exposes Agent after Coder without waiting for Controller", () => {
  const source = fs.readFileSync(startScript, "utf8");
  const coderReady = source.indexOf('if ($health.status -ne "ready") { throw "Coder runtime did not become ready" }');
  const agentStart = source.indexOf("$agent = Start-Process $node");
  const controllerProbe = source.indexOf('$controllerHealth = Invoke-RestMethod "http://127.0.0.1:3083/health"');

  assert.ok(coderReady >= 0, "Coder readiness gate is missing");
  assert.ok(agentStart > coderReady, "Agent must start after Coder is usable");
  assert.ok(controllerProbe > agentStart, "Controller may only be probed, not awaited before Agent starts");
  assert.match(source, /controllerStartedAt = \$controllerStartedAt/);
  assert.doesNotMatch(source.slice(0, agentStart), /TimeoutSeconds\s*=\s*420/);
});

test("incremental staging always refreshes the Agent protocol", () => {
  const source = fs.readFileSync(stageScript, "utf8");
  const alwaysRefreshMarker = source.indexOf("# Application code and startup policy are small release inputs");
  const agentCopy = source.indexOf('Copy-Directory (Join-Path $pocketmindRoot "agent")', alwaysRefreshMarker);
  const manifest = source.indexOf("$immutable = @(");
  assert.ok(alwaysRefreshMarker >= 0);
  assert.ok(agentCopy > alwaysRefreshMarker, "Agent code must refresh outside the optional runtime copy block");
  assert.ok(agentCopy < manifest, "Agent code must refresh before the integrity manifest is generated");
});

test("portable shutdown identifies capsule runtimes without WMI enumeration", () => {
  const source = fs.readFileSync(stopScript, "utf8");
  assert.match(source, /runtime\\node\\node\.exe/);
  assert.match(source, /runtime\\python\\python\.exe/);
  assert.match(source, /runtime\\llama\\b12211\\llama-server\.exe/);
  assert.match(source, /Get-Process -ErrorAction SilentlyContinue/);
  assert.doesNotMatch(source, /Get-CimInstance\s+Win32_Process/);
});

test("Controller lifecycle scripts are capsule-scoped and preserve core services", () => {
  const restart = fs.readFileSync(restartControllerScript, "utf8");
  const stop = fs.readFileSync(stopControllerScript, "utf8");
  for (const source of [restart, stop]) {
    assert.match(source, /runtime\\python\\python\.exe/);
    assert.match(source, /Get-Process -Id \$ProcessId/);
    assert.doesNotMatch(source, /Get-CimInstance\s+Win32_Process/);
    assert.doesNotMatch(source, /agentPid.*Stop-Process|coderPid.*Stop-Process|llamaPid.*Stop-Process/);
  }
  assert.match(restart, /controllerStartedAt/);
  assert.match(restart, /controllerTimedOutAt = \$null/);
  assert.match(stop, /controllerTimedOutAt/);
  assert.match(stop, /\$state\.modelPid = 0/);
});
