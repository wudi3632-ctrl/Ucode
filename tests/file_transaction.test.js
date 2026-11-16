import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { AuditLog } from "../agent/audit/audit.js";
import { AgentService } from "../agent/executor/service.js";
import { Store } from "../agent/store/store.js";
import { ProjectTools } from "../agent/tools/tools.js";


function faultingFileSystem({ failRenameAt, failRecovery = false }) {
  let renameCalls = 0;
  let injected = false;
  return {
    ...fs,
    renameSync(source, destination) {
      renameCalls += 1;
      if (!injected && renameCalls === failRenameAt) {
        injected = true;
        throw new Error("injected rename failure");
      }
      if (failRecovery && injected && String(source).includes(".ucode-") && String(source).endsWith(".bak")) {
        throw new Error("injected recovery failure");
      }
      return fs.renameSync(source, destination);
    },
  };
}

function fixture(fileSystem) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ucode-transaction-test-"));
  fs.writeFileSync(path.join(root, "first.txt"), "first-before\n", "utf8");
  fs.writeFileSync(path.join(root, "second.txt"), "second-before\n", "utf8");
  const tools = new ProjectTools({ timeoutMs: 1_000, maxOutputBytes: 100_000, fileSystem });
  const effect = tools.prepareBatch(root, { changes: [
    { path: "first.txt", operation: "modify", content: "first-after\n" },
    { path: "second.txt", operation: "modify", content: "second-after\n" },
    { path: "created.txt", operation: "create", content: "created-after\n" },
  ] });
  return { root, tools, effect };
}

test("mid-batch apply failure restores every original digest and removes transaction files", (t) => {
  const value = fixture(faultingFileSystem({ failRenameAt: 3 }));
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  assert.throws(() => value.tools.applyBatch(value.root, value.effect), (error) => error.code === "batch_apply_failed");
  assert.equal(fs.readFileSync(path.join(value.root, "first.txt"), "utf8"), "first-before\n");
  assert.equal(fs.readFileSync(path.join(value.root, "second.txt"), "utf8"), "second-before\n");
  assert.equal(fs.existsSync(path.join(value.root, "created.txt")), false);
  assert.equal(fs.readdirSync(value.root).some((name) => name.includes(".ucode-")), false);
  assert.equal(value.effect.status, "pending");
});

test("unverified recovery is reported distinctly and retains recoverable backup evidence", (t) => {
  const value = fixture(faultingFileSystem({ failRenameAt: 3, failRecovery: true }));
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  assert.throws(() => value.tools.applyBatch(value.root, value.effect), (error) => error.code === "batch_recovery_failed");
  const artifacts = fs.readdirSync(value.root).filter((name) => name.includes(".ucode-"));
  assert.ok(artifacts.some((name) => name.endsWith(".bak")));
  assert.equal(value.effect.status, "recovery_required");
});

test("Agent service stops a ticket and records recovery evidence instead of reopening approval", async (t) => {
  const fileSystem = faultingFileSystem({ failRenameAt: 3, failRecovery: true });
  const value = fixture(fileSystem);
  const data = path.join(value.root, "data");
  const audit = new AuditLog(data);
  const service = new AgentService({
    store: new Store(data),
    audit,
    tools: value.tools,
    model: { async generate() { throw new Error("unused"); } },
    dataDir: data,
  });
  service.selectProject(value.root);
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const ticket = service.createTicket("故障注入批量修改");
  const proposal = await service.callTool(ticket.id, "write_files", { changes: [
    { path: "first.txt", operation: "modify", content: "first-after\n" },
    { path: "second.txt", operation: "modify", content: "second-after\n" },
    { path: "created.txt", operation: "create", content: "created-after\n" },
  ] });

  assert.throws(() => service.approve(ticket.id, proposal.effect.id, true), (error) => error.code === "batch_recovery_failed");
  assert.equal(ticket.state, "failed");
  assert.equal(proposal.effect.status, "recovery_required");
  assert.equal(ticket.execution.pendingEffectId, null);
  assert.match(fs.readFileSync(path.join(data, "audit.jsonl"), "utf8"), /effect\.recovery_required/);
  assert.throws(() => service.continueTicket(ticket.id, "继续修改"), (error) => error.code === "recovery_required");
});
