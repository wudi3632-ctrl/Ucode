import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createHash, randomUUID } from "node:crypto";
import { AgentService, requestNegatesPath, resolveExplicitBatchTargets } from "../agent/executor/service.js";
import { AuditLog } from "../agent/audit/audit.js";
import { Store } from "../agent/store/store.js";
import { ProjectTools } from "../agent/tools/tools.js";

function fixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ucode-agent-test-"));
  const project = path.join(root, "project");
  const data = path.join(root, "data");
  fs.mkdirSync(path.join(project, "docs"), { recursive: true });
  fs.mkdirSync(path.join(project, ".pytest_cache"), { recursive: true });
  fs.writeFileSync(path.join(project, "README.md"), "# Paper Workspace\n\nThis project manages thesis chapters.", "utf8");
  fs.writeFileSync(path.join(project, "docs", "thesis_header.md"), "# Thesis Header\n\n研究背景与论文结构。", "utf8");
  fs.writeFileSync(path.join(project, "package.json"), '{"name":"paper-workspace"}', "utf8");
  fs.writeFileSync(path.join(project, ".pytest_cache", "README.md"), "generated cache", "utf8");
  const coder = {
    calls: [],
    async generate(payload) {
      this.calls.push(payload);
      return { type: "final", content: "这是一个论文写作项目，包含章节说明。" };
    },
    async generateStream(payload, options) {
      return this.generate(payload, options);
    },
  };
  const service = new AgentService({
    store: new Store(data),
    audit: new AuditLog(data),
    tools: new ProjectTools({ timeoutMs: 1_000, maxOutputBytes: 100_000 }),
    model: { async generate() { throw new Error("controller should not run"); } },
    coder,
    dataDir: data,
  });
  service.selectProject(project);
  return { root, project, data, service, coder };
}

function attachment(content, overrides = {}) {
  const bytes = Buffer.byteLength(content, "utf8");
  return {
    id: `attachment-${randomUUID()}`,
    displayName: "notes.txt",
    source: "external_readonly",
    relativePath: null,
    sizeBytes: bytes,
    includedBytes: bytes,
    truncated: false,
    sha256: createHash("sha256").update(content).digest("hex"),
    contentSha256: createHash("sha256").update(content).digest("hex"),
    content,
    ...overrides,
  };
}

test("injects approved skill context into system prompt without changing user message or route", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const ticket = value.service.createTicket("讲个简短笑话");
  value.service.setSkillContext(ticket.id, "回答必须先给结论，再给两个步骤。");
  const result = await value.service.plan(ticket.id);
  assert.equal(result.route, "chat");
  const call = value.coder.calls.at(-1);
  assert.match(call.messages[0].content, /已审批启用的本地技能说明/);
  assert.match(call.messages[0].content, /先给结论/);
  assert.equal(call.messages.findLast((message) => message.role === "user").content, "讲个简短笑话");
  assert.throws(() => value.service.setSkillContext(ticket.id, "x".repeat(51 * 1024)), /50 KiB/);
});

test("legacy chat exact-reply is deterministic and does not call the model", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  let attempt = 0;
  value.coder.generate = async (payload) => {
    value.coder.calls.push(payload);
    attempt += 1;
    return { type: "final", content: attempt === 1 ? "True" : "progressive_startup_ok" };
  };
  const events = [];
  const ticket = value.service.createTicket("只回答 progressive_startup_ok，不要读取或修改项目文件。");
  const result = await value.service.plan(ticket.id, { onEvent: (event) => events.push(event) });

  assert.equal(result.content, "progressive_startup_ok");
  assert.equal(attempt, 0);
  assert.deepEqual(events.filter((event) => event.type === "text_delta"), [{ type: "text_delta", delta: "progressive_startup_ok" }]);
  assert.doesNotMatch(JSON.stringify(events), /True/);
  assert.equal(result.evidence.length, 0);
});

test("broad tool work explains when the optional Controller is still loading", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  value.service.model.generate = async () => { throw Object.assign(new Error("connection refused"), { code: "model_unavailable" }); };
  const ticket = value.service.createTicket("全面分析项目架构并完成跨模块重构");

  await assert.rejects(
    () => value.service.plan(ticket.id),
    (error) => error.code === "controller_unavailable"
      && /普通对话和明确指定文件的任务仍可继续/.test(error.message),
  );
});

test("lists indexed files with successful evidence", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const ticket = value.service.createTicket("该目录有哪些文件");
  const result = await value.service.plan(ticket.id);
  assert.equal(result.route, "list_files");
  assert.match(result.content, /README\.md/);
  assert.equal(result.evidence[0].toolName, "list_files");
  assert.equal(result.evidence[0].status, "succeeded");
});

test("reads a named file and follows the same evidence in the next turn", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const ticket = value.service.createTicket("读取 thesis_header.md");
  const first = await value.service.plan(ticket.id);
  assert.match(first.content, /研究背景与论文结构/);
  assert.deepEqual(first.evidence[0].sourcePaths, ["docs/thesis_header.md"]);

  value.service.continueTicket(ticket.id, "它主要讲了什么");
  const second = await value.service.plan(ticket.id);
  assert.match(second.content, /来源/);
  assert.match(second.content, /docs\/thesis_header\.md/);
  assert.equal(value.coder.calls.at(-1).workspace.files[0].path, "docs/thesis_header.md");
});

test("returns named file content as a real fenced code block without asking the model", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const source = "def add(a, b):\n    return a + b\n";
  fs.writeFileSync(path.join(value.project, "calculator.py"), source, "utf8");
  const ticket = value.service.createTicket("calculator.py的内容");
  const result = await value.service.plan(ticket.id);

  assert.equal(result.route, "read_file");
  assert.equal(result.content, `\`calculator.py\` 的内容如下：\n\n\`\`\`python\n${source.trimEnd()}\n\`\`\``);
  assert.doesNotMatch(result.content, /\\n/);
  assert.equal(value.coder.calls.length, 0);
});

test("describes a named source file from real symbols without a template model answer", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  fs.writeFileSync(path.join(value.project, "calculator.py"), "def add(a, b):\n    return a + b\n", "utf8");
  const ticket = value.service.createTicket("calculator.py 是什么文件");
  const result = await value.service.plan(ticket.id);

  assert.equal(result.route, "read_file");
  assert.match(result.content, /Python 源文件/);
  assert.match(result.content, /`add`/);
  assert.equal(value.coder.calls.length, 0);
});

test("does not append a second source section when the model already included one", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  value.coder.generate = async (payload) => {
    value.coder.calls.push(payload);
    return { type: "final", content: "README 描述了论文工作区。\n\n来源\n- `README.md`" };
  };
  const ticket = value.service.createTicket("README.md 主要说明了什么？");
  const result = await value.service.plan(ticket.id);

  assert.equal((result.content.match(/\n来源\n/g) || []).length, 1);
  assert.equal((result.content.match(/- `README\.md`/g) || []).length, 1);
});

test("uses a stable real file for an arbitrary-read follow-up", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const ticket = value.service.createTicket("那你随便读取一个试试");
  const result = await value.service.plan(ticket.id);
  assert.match(result.content, /已读取 `README\.md`/);
  assert.deepEqual(result.evidence[0].sourcePaths, ["README.md"]);
});

test("excludes generated cache directories from project evidence", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const ticket = value.service.createTicket("该目录有哪些文件");
  const result = await value.service.plan(ticket.id);
  assert.doesNotMatch(result.content, /\.pytest_cache/);
  assert.ok(result.evidence[0].sourcePaths.every((file) => !file.startsWith(".pytest_cache/")));
});

test("reports an explicitly requested missing path", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const ticket = value.service.createTicket("读取 missing_file.md");
  const result = await value.service.plan(ticket.id);
  assert.equal(result.type, "needs_clarification");
  assert.match(result.content, /项目索引中不存在 `missing_file\.md`/);
  assert.equal(result.evidence.length, 0);
});

test("summarizes only after representative files were actually read", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const ticket = value.service.createTicket("总结本项目");
  const result = await value.service.plan(ticket.id);
  assert.equal(result.route, "summarize_project");
  assert.ok(result.evidence.length >= 2);
  assert.ok(result.evidence.every((item) => item.toolName === "read_file" && item.status === "succeeded"));
  assert.match(result.content, /README\.md/);
  assert.ok(value.coder.calls[0].workspace.files.every((file) => file.content.length > 0));
});

test("answers an exact project question only after recording real file evidence", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const events = [];
  const ticket = value.service.createTicket("README.md 主要说明了什么？");
  const result = await value.service.plan(ticket.id, { onEvent(event) { events.push(event); } });

  assert.equal(result.route, "project_question");
  assert.deepEqual(result.evidence.map((item) => item.sourcePaths[0]), ["README.md"]);
  assert.match(result.content, /来源/);
  assert.match(result.content, /README\.md/);
  assert.ok(events.some((event) => event.type === "tool_completed" && event.tool === "read_file"));
  const call = value.coder.calls.at(-1);
  assert.deepEqual(call.tools, []);
  assert.match(call.workspace.files.find((file) => file.path === "README.md").content, /Paper Workspace/);
});

test("retries a project answer once as strict plain text after raw tool output", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  let attempts = 0;
  value.coder.generate = async (payload) => {
    value.coder.calls.push(payload);
    attempts += 1;
    return attempts === 1
      ? { type: "final", content: '{"name":"read_file","arguments":{"path":"README.md"}}' }
      : { type: "final", content: "README 描述了论文工作区。" };
  };
  const ticket = value.service.createTicket("README.md 是做什么的？");
  const result = await value.service.plan(ticket.id);

  assert.equal(attempts, 2);
  assert.match(result.content, /论文工作区/);
  assert.match(value.coder.calls[1].messages[0].content, /上一次输出没有遵守纯文本回答协议/);
  assert.doesNotMatch(value.service.ticket(ticket.id).execution.messages.at(-1).content, /read_file/);
});

test("fails safely when both project-answer attempts are tool-shaped", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  value.coder.generate = async () => ({ type: "tool_request", content: "", tool: { name: "read_file", arguments: { path: "README.md" } } });
  const ticket = value.service.createTicket("解释 README.md 的用途");

  await assert.rejects(value.service.plan(ticket.id), /没有生成可用的项目回答/);
  assert.equal(value.service.ticket(ticket.id).state, "failed");
  assert.ok(value.service.ticket(ticket.id).execution.messages.every((message) => !message.tool_calls));
});

test("clarifies project questions that have no real file target", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const ticket = value.service.createTicket("为什么这个项目会报错？");
  const result = await value.service.plan(ticket.id);

  assert.equal(result.type, "needs_clarification");
  assert.equal(result.evidence.length, 0);
  assert.match(result.content, /请指定文件名/);
  assert.equal(value.coder.calls.length, 0);
});

test("does not mark a mutation complete without a reviewed effect", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  value.service.model.generate = async () => ({ type: "final", content: "已经修复完成。" });
  const ticket = value.service.createTicket("修复这个项目中的错误");
  const result = await value.service.plan(ticket.id);

  assert.equal(result.type, "needs_clarification");
  assert.equal(value.service.ticket(ticket.id).state, "waiting");
  assert.match(result.content, /没有生成可审查的修改 Diff/);
  assert.doesNotMatch(result.content, /已经修复完成/);
});

test("read evidence alone cannot prove that a requested mutation completed", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  let turn = 0;
  value.service.model.generate = async () => {
    turn += 1;
    return turn === 1
      ? { type: "tool_request", content: "", tool: { name: "read_file", arguments: { path: "README.md" } } }
      : { type: "final", content: "修改完成。" };
  };
  const ticket = value.service.createTicket("修复这个项目中的错误");
  const result = await value.service.plan(ticket.id);

  assert.equal(result.type, "needs_clarification");
  assert.equal(result.evidence[0].toolName, "read_file");
  assert.match(result.content, /没有生成可审查的修改 Diff/);
});

test("allows a pytest-only task to complete only after real test evidence", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  value.service.tools.pytest = async () => ({ exitCode: 0, signal: null, stdout: "2 passed", stderr: "" });
  let turn = 0;
  value.service.model.generate = async () => {
    turn += 1;
    return turn === 1
      ? { type: "tool_request", content: "", tool: { name: "pytest", arguments: { paths: [] } } }
      : { type: "final", content: "测试通过。" };
  };
  const ticket = value.service.createTicket("运行 pytest");
  const result = await value.service.plan(ticket.id);

  assert.equal(result.type, "final");
  assert.equal(result.evidence.at(-1).toolName, "pytest");
  assert.equal(result.evidence.at(-1).exitCode, 0);
  assert.equal(value.service.ticket(ticket.id).state, "ended");
});

test("does not claim a project operation when no file target is resolved", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const ticket = value.service.createTicket("读取该项目的内容");
  const result = await value.service.plan(ticket.id);
  assert.equal(result.type, "needs_clarification");
  assert.equal(result.evidence.length, 0);
  assert.doesNotMatch(result.content, /已读取/);
});

test("grounds one turn with an external read-only attachment without persisting its content", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const marker = `PRIVATE_ATTACHMENT_${randomUUID()}`;
  const ticket = value.service.createTicket("这个附件主要讲了什么？");
  value.service.setAttachmentContext(ticket.id, [attachment(`会议结论：${marker}`)]);
  const result = await value.service.plan(ticket.id);
  const workspaceAttachment = value.coder.calls.at(-1).workspace.files.find((file) => file.attachment);
  assert.equal(workspaceAttachment.content, `会议结论：${marker}`);
  assert.match(workspaceAttachment.path, /^attachment:\/\/external\//);
  assert.equal(result.evidence[0].toolName, "attachment_read");
  assert.equal(result.evidence[0].preview, null);
  const persisted = fs.readdirSync(value.data).map((name) => fs.readFileSync(path.join(value.data, name), "utf8")).join("\n");
  assert.doesNotMatch(persisted, new RegExp(marker));

  value.service.continueTicket(ticket.id, "再概括一次");
  await value.service.plan(ticket.id);
  assert.ok(value.coder.calls.at(-1).workspace.files.every((file) => !file.attachment));

  value.service.continueTicket(ticket.id, "本轮是否仍有附件？");
  const status = await value.service.plan(ticket.id);
  assert.match(status.content, /本轮没有附件/);
  assert.equal(status.evidence.length, 0);
});

test("redacts attachment-derived model text from the audit log", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const marker = `DERIVED_SECRET_${randomUUID()}`;
  value.coder.generate = async (payload) => {
    value.coder.calls.push(payload);
    return { type: "final", content: marker };
  };
  const ticket = value.service.createTicket("复述附件内容");
  value.service.setAttachmentContext(ticket.id, [attachment(marker)]);
  const result = await value.service.plan(ticket.id);
  assert.match(result.content, new RegExp(marker));
  const audit = fs.readFileSync(path.join(value.data, "audit.jsonl"), "utf8");
  assert.doesNotMatch(audit, new RegExp(marker));
  assert.match(audit, /contentSha256/);
});

test("rejects forged attachment metadata and unsafe project paths", (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const ticket = value.service.createTicket("分析附件");
  assert.throws(() => value.service.setAttachmentContext(ticket.id, [attachment("safe", { contentSha256: "0".repeat(64) })]), /digest/i);
  assert.throws(() => value.service.setAttachmentContext(ticket.id, [attachment("safe", { source: "project", relativePath: "../outside.txt" })]), /path/i);
  assert.throws(() => value.service.setAttachmentContext(ticket.id, [attachment("x".repeat(120_001))]), /120,000/);
});

test("never exposes attachments as tool paths or overwrites a same-named project file", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const ticket = value.service.createTicket("修改 notes.txt 的内容");
  const item = attachment("external-only", { displayName: "README.md" });
  value.service.setAttachmentContext(ticket.id, [item]);
  value.service.model.generate = async () => ({
    type: "tool_request",
    content: "",
    tool: { name: "write_file", arguments: { path: `attachment://external/${item.id}/README.md`, content: "changed" } },
  });
  await assert.rejects(value.service.plan(ticket.id), /无效或越界路径/);
  assert.equal(fs.readFileSync(path.join(value.project, "README.md"), "utf8"), "# Paper Workspace\n\nThis project manages thesis chapters.");

  const second = value.service.createTicket("比较这个附件和 README.md");
  value.service.setAttachmentContext(second.id, [item]);
  await value.service.plan(second.id);
  const files = value.coder.calls.at(-1).workspace.files;
  assert.ok(files.some((file) => file.path === "README.md" && !file.attachment));
  assert.ok(files.some((file) => file.path.startsWith("attachment://external/") && file.attachment));
});

test("repairs one unsafe generated Python path and prevents the coder from redirecting it", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  value.service.model.generate = async () => ({
    type: "tool_request",
    content: "",
    tool: { name: "write_file", arguments: { path: "b/../b/../dynamic_programming.py", content: "placeholder" } },
  });
  value.coder.generate = async (payload) => {
    value.coder.calls.push(payload);
    return {
      type: "tool_request",
      content: "",
      tool: { name: "write_file", arguments: { path: "C:/unsafe/output.py", content: "def solve(values):\n    return max(values, default=0)\n" } },
    };
  };
  const ticket = value.service.createTicket("创建一个动态规划 Python 文件");
  const result = await value.service.plan(ticket.id);

  assert.equal(result.effect.status, "pending");
  assert.equal(result.effect.path, "dynamic_programming.py");
  assert.equal(result.tool.arguments.path, "dynamic_programming.py");
  assert.equal(value.coder.calls[0].requested_tool.arguments.path, "dynamic_programming.py");
  assert.equal(fs.existsSync(path.join(value.project, "dynamic_programming.py")), false);
  assert.equal(fs.existsSync(path.join(value.project, "output.py")), false);
});

test("completes a safe additive Python snippet into a full-file write proposal", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const original = "def add(a, b):\n    return a + b\n";
  fs.writeFileSync(path.join(value.project, "calculator.py"), original, "utf8");
  value.coder.generate = async (payload) => {
    value.coder.calls.push(payload);
    return {
      type: "tool_request",
      content: "",
      tool: { name: "write_file", arguments: { path: "calculator.py", content: "[write_file] calculator.py def multiply(a, b): return a * b\n" } },
    };
  };
  const ticket = value.service.createTicket("修改 calculator.py：新增 multiply(a, b) 函数");
  const result = await value.service.plan(ticket.id);
  assert.equal(result.effect.status, "pending");
  assert.match(result.effect.content, /def add/);
  assert.match(result.effect.content, /def multiply/);
  assert.equal(fs.readFileSync(path.join(value.project, "calculator.py"), "utf8"), original);
  value.service.approve(ticket.id, result.effect.id, true);
  const completed = await value.service.plan(ticket.id);
  assert.equal(completed.type, "final");
  assert.match(completed.content, /已应用/);
  assert.match(fs.readFileSync(path.join(value.project, "calculator.py"), "utf8"), /def multiply/);
});

test("runs requested pytest after approval before declaring an explicit edit complete", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const original = "def add(a, b):\n    return a + b\n";
  fs.writeFileSync(path.join(value.project, "calculator.py"), original, "utf8");
  value.coder.generate = async () => ({
    type: "tool_request",
    content: "",
    tool: { name: "write_file", arguments: { path: "calculator.py", content: `${original}\n\ndef multiply(a, b):\n    return a * b\n` } },
  });
  let pytestCalls = 0;
  value.service.tools.pytest = async () => {
    pytestCalls += 1;
    return { exitCode: 0, signal: null, stdout: "3 passed", stderr: "" };
  };
  const ticket = value.service.createTicket("修改 calculator.py：新增 multiply(a, b)，然后运行 pytest");
  const proposal = await value.service.plan(ticket.id);
  value.service.approve(ticket.id, proposal.effect.id, true);
  const completed = await value.service.plan(ticket.id);

  assert.equal(pytestCalls, 1);
  assert.equal(completed.type, "final");
  assert.equal(completed.route, "verified_edit");
  assert.match(completed.content, /pytest 已通过/);
  assert.equal(completed.evidence.at(-1).toolName, "pytest");
});

test("shows a bounded sanitized pytest failure summary after an approved edit", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const original = "def add(a, b):\n    return a + b\n";
  fs.writeFileSync(path.join(value.project, "calculator.py"), original, "utf8");
  value.coder.generate = async () => ({
    type: "tool_request",
    content: "",
    tool: { name: "write_file", arguments: { path: "calculator.py", content: `${original}\n\ndef multiply(a, b):\n    return a * b\n` } },
  });
  const noisyFailure = `\u001b[31m.FF\u001b[0m\nFAILURES\n${value.project}\\tests\\test_calculator.py:12\ntest_checkout_total\n${"extra diagnostic line\n".repeat(30)}AssertionError: assert 100 == 110\n1 failed, 2 passed`;
  value.service.tools.pytest = async () => ({ exitCode: 1, signal: null, stdout: noisyFailure, stderr: "" });

  const ticket = value.service.createTicket("修改 calculator.py：新增 multiply(a, b)，然后运行 pytest");
  const proposal = await value.service.plan(ticket.id);
  value.service.approve(ticket.id, proposal.effect.id, true);
  const completed = await value.service.plan(ticket.id);

  assert.equal(completed.type, "needs_clarification");
  assert.equal(completed.route, "verified_edit");
  assert.match(completed.content, /pytest 未通过（退出码 1）/);
  assert.match(completed.content, /测试摘要：/);
  assert.match(completed.content, /test_checkout_total/);
  assert.match(completed.content, /AssertionError: assert 100 == 110/);
  assert.doesNotMatch(completed.content, /\u001b\[31m/);
  assert.doesNotMatch(completed.content, new RegExp(value.project.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  assert.ok(completed.content.length < 800);
  assert.match(completed.content, /继续修复，或回滚/);
});

test("a failed pytest follow-up proposes a new reviewed repair before rerunning tests", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const original = "def add(a, b):\n    return a + b\n";
  const firstCandidate = `${original}\n\ndef multiply(a, b):\n    return a - b\n`;
  const repairedCandidate = `${original}\n\ndef multiply(a, b):\n    return a * b\n`;
  fs.writeFileSync(path.join(value.project, "calculator.py"), original, "utf8");
  fs.mkdirSync(path.join(value.project, "tests"), { recursive: true });
  fs.writeFileSync(path.join(value.project, "tests", "test_calculator.py"), "from calculator import multiply\n\ndef test_multiply():\n    assert multiply(2, 3) == 6\n", "utf8");
  let coderCalls = 0;
  value.coder.generate = async (payload) => {
    value.coder.calls.push(payload);
    coderCalls += 1;
    return {
      type: "tool_request",
      content: "",
      tool: { name: "write_file", arguments: { path: "calculator.py", content: coderCalls === 1 ? firstCandidate : repairedCandidate } },
    };
  };
  let pytestCalls = 0;
  value.service.tools.pytest = async () => {
    pytestCalls += 1;
    return pytestCalls === 1
      ? { exitCode: 1, signal: null, stdout: ".F\nFAILURES\n________ test_multiply ________\ntests/test_calculator.py:4\n> assert multiply(2, 3) == 6\nE AssertionError: assert -1 == 6\nFAILED tests/test_calculator.py::test_multiply", stderr: "" }
      : { exitCode: 0, signal: null, stdout: "4 passed", stderr: "" };
  };

  const ticket = value.service.createTicket("修改 calculator.py：新增 multiply(a, b)，然后运行 pytest");
  const firstProposal = await value.service.plan(ticket.id);
  value.service.approve(ticket.id, firstProposal.effect.id, true);
  const failed = await value.service.plan(ticket.id);
  assert.equal(failed.type, "needs_clarification");
  assert.equal(pytestCalls, 1);

  value.service.continueTicket(ticket.id, "根据测试结果继续修复，并再次运行 pytest");
  const repairProposal = await value.service.plan(ticket.id);
  assert.equal(repairProposal.type, "message");
  assert.equal(repairProposal.route, "failed_test_repair");
  assert.equal(repairProposal.effect.status, "pending");
  assert.match(repairProposal.content, /下一轮修复 Diff/);
  assert.equal(pytestCalls, 1, "tests must not rerun before the repair Diff is approved");
  assert.equal(fs.readFileSync(path.join(value.project, "calculator.py"), "utf8"), firstCandidate);
  const repairCall = value.coder.calls.at(-1);
  assert.deepEqual(repairCall.workspace.files.map((file) => file.path), ["calculator.py", "tests/test_calculator.py"]);
  assert.equal(repairCall.workspace.files[1].readOnly, true);
  assert.equal(repairCall.requested_tool.arguments.path, "calculator.py");
  assert.match(repairCall.messages[0].content, /其他工作区文件只是已证明的只读测试上下文/);
  assert.match(repairCall.messages[1].content, /失败用例：test_multiply/);
  assert.match(repairCall.messages[1].content, /实际值：-1/);
  assert.match(repairCall.messages[1].content, /期望值：6/);

  value.service.approve(ticket.id, repairProposal.effect.id, true);
  const completed = await value.service.plan(ticket.id);
  assert.equal(completed.type, "final");
  assert.match(completed.content, /pytest 已通过/);
  assert.equal(pytestCalls, 2);
  assert.equal(fs.readFileSync(path.join(value.project, "calculator.py"), "utf8"), repairedCandidate);
});

test("a failed pytest follow-up ignores unindexed and escaping test paths", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const original = "def add(a, b):\n    return a + b\n";
  const firstCandidate = `${original}\n\ndef multiply(a, b):\n    return a - b\n`;
  const repairedCandidate = `${original}\n\ndef multiply(a, b):\n    return a * b\n`;
  fs.writeFileSync(path.join(value.project, "calculator.py"), original, "utf8");
  let coderCalls = 0;
  value.coder.generate = async (payload) => {
    value.coder.calls.push(payload);
    coderCalls += 1;
    return { type: "tool_request", content: "", tool: { name: "write_file", arguments: { path: "calculator.py", content: coderCalls === 1 ? firstCandidate : repairedCandidate } } };
  };
  value.service.tools.pytest = async () => ({
    exitCode: 1,
    signal: null,
    stdout: ".F\nFAILURES\n../../outside/test_evil.py:9\nFAILED ../../outside/test_evil.py::test_escape\nAssertionError: assert 1 == 2",
    stderr: "",
  });

  const ticket = value.service.createTicket("修改 calculator.py：新增 multiply(a, b)，然后运行 pytest");
  const proposal = await value.service.plan(ticket.id);
  value.service.approve(ticket.id, proposal.effect.id, true);
  await value.service.plan(ticket.id);
  value.service.continueTicket(ticket.id, "根据测试结果继续修复，并再次运行 pytest");
  await value.service.plan(ticket.id);

  const repairCall = value.coder.calls.at(-1);
  assert.deepEqual(repairCall.workspace.files.map((file) => file.path), ["calculator.py"]);
  assert.doesNotMatch(repairCall.messages[1].content, /已证明的测试文件/);
  assert.equal(repairCall.requested_tool.arguments.path, "calculator.py");
});

test("an invalid test-repair candidate waits for better input without rerunning pytest", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const original = "def add(a, b):\n    return a + b\n";
  const firstCandidate = `${original}\n\ndef multiply(a, b):\n    return a - b\n`;
  fs.writeFileSync(path.join(value.project, "calculator.py"), original, "utf8");
  let coderCalls = 0;
  value.coder.generate = async () => {
    coderCalls += 1;
    if (coderCalls === 1) return { type: "tool_request", content: "", tool: { name: "write_file", arguments: { path: "calculator.py", content: firstCandidate } } };
    return { type: "message", content: "这里应该修改 multiply" };
  };
  let pytestCalls = 0;
  value.service.tools.pytest = async () => {
    pytestCalls += 1;
    return { exitCode: 1, signal: null, stdout: ".F\nFAILURES\ntest_multiply\nAssertionError: assert -1 == 6", stderr: "" };
  };

  const ticket = value.service.createTicket("修改 calculator.py：新增 multiply(a, b)，然后运行 pytest");
  const proposal = await value.service.plan(ticket.id);
  value.service.approve(ticket.id, proposal.effect.id, true);
  await value.service.plan(ticket.id);
  value.service.continueTicket(ticket.id, "根据测试结果继续修复，并再次运行 pytest");
  const waiting = await value.service.plan(ticket.id);

  assert.equal(waiting.type, "needs_clarification");
  assert.equal(waiting.route, "failed_test_repair");
  assert.match(waiting.content, /没有写入，也没有重复运行 pytest/);
  assert.equal(pytestCalls, 1);
  assert.equal(value.service.ticket(ticket.id).effects.length, 1);
  assert.equal(fs.readFileSync(path.join(value.project, "calculator.py"), "utf8"), firstCandidate);
});

test("a failed-test repair retries a no-op candidate once and accepts a real change", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const original = "def add(a, b):\n    return a + b\n";
  const firstCandidate = `${original}\n\ndef multiply(a, b):\n    return a - b\n`;
  const repairedCandidate = `${original}\n\ndef multiply(a, b):\n    return a * b\n`;
  fs.writeFileSync(path.join(value.project, "calculator.py"), original, "utf8");
  let coderCalls = 0;
  value.coder.generate = async (payload) => {
    value.coder.calls.push(payload);
    coderCalls += 1;
    const content = coderCalls === 1 ? firstCandidate : coderCalls === 2 ? firstCandidate : repairedCandidate;
    return { type: "tool_request", content: "", tool: { name: "write_file", arguments: { path: "calculator.py", content } } };
  };
  let pytestCalls = 0;
  value.service.tools.pytest = async () => {
    pytestCalls += 1;
    return { exitCode: 1, signal: null, stdout: ".F\nFAILURES\ntest_multiply\nAssertionError: assert -1 == 6", stderr: "" };
  };

  const ticket = value.service.createTicket("修改 calculator.py：新增 multiply(a, b)，然后运行 pytest");
  const proposal = await value.service.plan(ticket.id);
  value.service.approve(ticket.id, proposal.effect.id, true);
  await value.service.plan(ticket.id);
  value.service.continueTicket(ticket.id, "根据测试结果继续修复，并再次运行 pytest");
  const repair = await value.service.plan(ticket.id);

  assert.equal(repair.type, "message");
  assert.equal(repair.effect.changes[0].content, repairedCandidate);
  assert.equal(value.service.ticket(ticket.id).effects.length, 2);
  assert.equal(pytestCalls, 1);
  assert.match(value.coder.calls.at(-1).messages.at(-1).content, /没有修复任何失败/);
  assert.equal(fs.readFileSync(path.join(value.project, "calculator.py"), "utf8"), firstCandidate);
});

test("two no-op failed-test repair candidates create no Effect or pytest rerun", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const original = "def add(a, b):\n    return a + b\n";
  const firstCandidate = `${original}\n\ndef multiply(a, b):\n    return a - b\n`;
  fs.writeFileSync(path.join(value.project, "calculator.py"), original, "utf8");
  let coderCalls = 0;
  value.coder.generate = async (payload) => {
    value.coder.calls.push(payload);
    coderCalls += 1;
    const content = coderCalls === 1 ? firstCandidate : coderCalls === 2 ? firstCandidate : firstCandidate.replace(/\n/g, "\r\n");
    return { type: "tool_request", content: "", tool: { name: "write_file", arguments: { path: "calculator.py", content } } };
  };
  let pytestCalls = 0;
  value.service.tools.pytest = async () => {
    pytestCalls += 1;
    return { exitCode: 1, signal: null, stdout: ".F\nFAILURES\ntest_multiply\nAssertionError: assert -1 == 6", stderr: "" };
  };

  const ticket = value.service.createTicket("修改 calculator.py：新增 multiply(a, b)，然后运行 pytest");
  const proposal = await value.service.plan(ticket.id);
  value.service.approve(ticket.id, proposal.effect.id, true);
  await value.service.plan(ticket.id);
  value.service.continueTicket(ticket.id, "根据测试结果继续修复，并再次运行 pytest");
  const waiting = await value.service.plan(ticket.id);

  assert.equal(waiting.type, "needs_clarification");
  assert.equal(waiting.route, "failed_test_repair");
  assert.match(waiting.content, /连续两次都没有.*有效的实现变化/);
  assert.match(waiting.content, /更换更强模型/);
  assert.deepEqual(waiting.recovery, {
    reason: "candidate_no_change",
    path: "calculator.py",
    attempts: 2,
    actions: ["attach_context", "configure_model"],
  });
  assert.equal(value.service.ticket(ticket.id).effects.length, 1);
  assert.equal(value.service.ticket(ticket.id).execution.pendingEffectId, null);
  assert.equal(pytestCalls, 1);
  assert.equal(fs.readFileSync(path.join(value.project, "calculator.py"), "utf8"), firstCandidate);
});

test("failed-test repair rejects comment-only candidates as having no implementation effect", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const original = "def add(a, b):\n    return a + b\n";
  const firstCandidate = `${original}\n\ndef multiply(a, b):\n    return a - b\n`;
  fs.writeFileSync(path.join(value.project, "calculator.py"), original, "utf8");
  let coderCalls = 0;
  value.coder.generate = async (payload) => {
    value.coder.calls.push(payload);
    coderCalls += 1;
    const content = coderCalls === 1 ? firstCandidate : coderCalls === 2 ? `# calculator.py\n${firstCandidate}` : `# attempted repair\n${firstCandidate}`;
    return { type: "tool_request", content: "", tool: { name: "write_file", arguments: { path: "calculator.py", content } } };
  };
  let pytestCalls = 0;
  value.service.tools.pytest = async () => {
    pytestCalls += 1;
    return { exitCode: 1, signal: null, stdout: ".F\nFAILURES\ntest_multiply\nAssertionError: assert -1 == 6", stderr: "" };
  };

  const ticket = value.service.createTicket("修改 calculator.py：新增 multiply(a, b)，然后运行 pytest");
  const proposal = await value.service.plan(ticket.id);
  value.service.approve(ticket.id, proposal.effect.id, true);
  await value.service.plan(ticket.id);
  value.service.continueTicket(ticket.id, "根据测试结果继续修复，并再次运行 pytest");
  const waiting = await value.service.plan(ticket.id);

  assert.equal(waiting.type, "needs_clarification");
  assert.equal(waiting.recovery.reason, "candidate_no_effect");
  assert.match(waiting.content, /没有.*有效的实现变化/);
  assert.equal(value.service.ticket(ticket.id).effects.length, 1);
  assert.equal(pytestCalls, 1);
  assert.equal(fs.readFileSync(path.join(value.project, "calculator.py"), "utf8"), firstCandidate);
});

test("routes an additive request with an explicit filename directly to the coder", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const original = "def solve():\n    return 1\n";
  fs.writeFileSync(path.join(value.project, "knapsack.py"), original, "utf8");
  value.coder.generate = async () => { throw new Error("a deterministic top-comment edit must not call the model"); };
  const ticket = value.service.createTicket("在 knapsack.py 顶部增加注释 # review，只生成待审批 Diff");
  const result = await value.service.plan(ticket.id);
  assert.equal(result.route, "explicit_edit");
  assert.equal(result.effect.path, "knapsack.py");
  assert.equal(result.effect.status, "pending");
  assert.equal(result.effect.content, `# review\n${original}`);
  assert.equal(fs.readFileSync(path.join(value.project, "knapsack.py"), "utf8"), original);
});

test("a rejected deterministic edit finishes with no file write", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const original = "def solve():\n    return 1\n";
  fs.writeFileSync(path.join(value.project, "solver.py"), original, "utf8");
  const ticket = value.service.createTicket("在 solver.py 顶部增加注释 # reviewed");
  const proposal = await value.service.plan(ticket.id);
  assert.equal(proposal.effect.status, "pending");
  value.service.approve(ticket.id, proposal.effect.id, false);
  const completed = await value.service.plan(ticket.id);
  assert.equal(completed.type, "final");
  assert.match(completed.content, /已取消/);
  assert.equal(fs.readFileSync(path.join(value.project, "solver.py"), "utf8"), original);
});

test("applies and rolls back a selected multi-file batch as one reviewed Effect", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const readmeBefore = fs.readFileSync(path.join(value.project, "README.md"), "utf8");
  const packageBefore = fs.readFileSync(path.join(value.project, "package.json"), "utf8");
  const ticket = value.service.createTicket("同时更新说明、新建记录并删除旧清单");
  const proposal = await value.service.callTool(ticket.id, "write_files", {
    changes: [
      { path: "README.md", operation: "modify", content: `${readmeBefore}\n\nReviewed batch.\n` },
      { path: "docs/batch-note.md", operation: "create", content: "# Batch note\n" },
      { path: "package.json", operation: "delete" },
    ],
  });

  assert.equal(proposal.effect.changes.length, 3);
  assert.equal(fs.readFileSync(path.join(value.project, "README.md"), "utf8"), readmeBefore);
  assert.equal(fs.existsSync(path.join(value.project, "docs", "batch-note.md")), false);
  assert.equal(fs.readFileSync(path.join(value.project, "package.json"), "utf8"), packageBefore);

  const applied = value.service.approve(ticket.id, proposal.effect.id, true, ["README.md", "docs/batch-note.md", "package.json"]);
  assert.deepEqual(applied.paths, ["README.md", "docs/batch-note.md", "package.json"]);
  assert.match(fs.readFileSync(path.join(value.project, "README.md"), "utf8"), /Reviewed batch/);
  assert.equal(fs.readFileSync(path.join(value.project, "docs", "batch-note.md"), "utf8"), "# Batch note\n");
  assert.equal(fs.existsSync(path.join(value.project, "package.json")), false);

  const rolledBack = value.service.rollback(ticket.id, proposal.effect.id);
  assert.deepEqual(rolledBack.paths, ["README.md", "docs/batch-note.md", "package.json"]);
  assert.equal(fs.readFileSync(path.join(value.project, "README.md"), "utf8"), readmeBefore);
  assert.equal(fs.existsSync(path.join(value.project, "docs", "batch-note.md")), false);
  assert.equal(fs.readFileSync(path.join(value.project, "package.json"), "utf8"), packageBefore);
});

test("refuses the entire multi-file batch when one reviewed digest is stale", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const readmeBefore = fs.readFileSync(path.join(value.project, "README.md"), "utf8");
  const headerPath = path.join(value.project, "docs", "thesis_header.md");
  const headerBefore = fs.readFileSync(headerPath, "utf8");
  const ticket = value.service.createTicket("同时更新两个文档");
  const proposal = await value.service.callTool(ticket.id, "write_files", {
    changes: [
      { path: "README.md", operation: "modify", content: `${readmeBefore}\nNext\n` },
      { path: "docs/thesis_header.md", operation: "modify", content: `${headerBefore}\nNext\n` },
    ],
  });
  fs.writeFileSync(headerPath, `${headerBefore}\nExternal change\n`, "utf8");

  assert.throws(
    () => value.service.approve(ticket.id, proposal.effect.id, true),
    /File changed after approval was requested/,
  );
  assert.equal(fs.readFileSync(path.join(value.project, "README.md"), "utf8"), readmeBefore);
  assert.match(fs.readFileSync(headerPath, "utf8"), /External change/);
  assert.equal(proposal.effect.status, "pending");
  assert.equal(fs.readdirSync(value.project).some((name) => name.includes(".ucode-")), false);
});

test("controller can plan paths while coder supplies each non-delete batch file", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  value.service.model.generate = async () => ({
    type: "tool_request",
    content: "",
    tool: { name: "write_files", arguments: { changes: [
      { path: "README.md", operation: "modify" },
      { path: "docs/generated.md", operation: "create" },
      { path: "package.json", operation: "delete" },
    ] } },
  });
  value.coder.generate = async (payload) => {
    const pathname = payload.requested_tool.arguments.path;
    return { type: "tool_request", content: "", tool: { name: "write_file", arguments: {
      path: pathname,
      content: pathname === "README.md" ? "# Paper Workspace\n\nUpdated by batch.\n" : "# Generated\n",
    } } };
  };
  const ticket = value.service.createTicket("根据发布计划修改项目文件并完成一组新增和删除操作");
  const result = await value.service.plan(ticket.id);

  assert.equal(result.effect.changes.length, 3);
  assert.equal(result.effect.changes[0].content, "# Paper Workspace\n\nUpdated by batch.\n");
  assert.equal(result.effect.changes[1].content, "# Generated\n");
  assert.equal(result.effect.changes[2].content, null);
  assert.equal(fs.existsSync(path.join(value.project, "docs", "generated.md")), false);
  assert.equal(fs.existsSync(path.join(value.project, "package.json")), true);
});

test("explicit multi-file edits bypass a looping controller and produce one approval batch", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  fs.writeFileSync(path.join(value.project, "batch_alpha.py"), 'def alpha():\n    return "old-alpha"\n', "utf8");
  fs.writeFileSync(path.join(value.project, "batch_beta.py"), 'def beta():\n    return "old-beta"\n', "utf8");
  let controllerCalls = 0;
  value.service.model.generate = async () => {
    controllerCalls += 1;
    return { type: "tool_request", content: "", tool: { name: "read_file", arguments: { path: "batch_alpha.py" } } };
  };
  value.coder.generate = async (payload) => {
    const pathname = payload.requested_tool.arguments.path;
    const content = pathname === "batch_alpha.py"
      ? 'def alpha():\n    return "new-alpha"\n'
      : 'def beta():\n    return "new-beta"\n';
    return { type: "tool_request", content: "", tool: { name: "write_file", arguments: { path: pathname, content } } };
  };

  const ticket = value.service.createTicket("同时修改 batch_alpha.py 和 batch_beta.py，把 alpha() 的返回值改成 new-alpha，把 beta() 的返回值改成 new-beta");
  const result = await value.service.plan(ticket.id);

  assert.equal(controllerCalls, 0);
  assert.equal(result.route, "explicit_batch_edit");
  assert.equal(result.effect.status, "pending");
  assert.deepEqual(result.effect.changes.map((change) => change.path), ["batch_alpha.py", "batch_beta.py"]);
  assert.equal(fs.readFileSync(path.join(value.project, "batch_alpha.py"), "utf8"), 'def alpha():\n    return "old-alpha"\n');
  assert.equal(fs.readFileSync(path.join(value.project, "batch_beta.py"), "utf8"), 'def beta():\n    return "old-beta"\n');
});

test("explicit three-file literal edits stay scoped and bypass the small coder", async (t) => {
  assert.equal(requestNegatesPath("不能修改 plugins/alpha/config.py", "plugins/alpha/config.py"), true);
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  fs.mkdirSync(path.join(value.project, "ucodebench"), { recursive: true });
  fs.mkdirSync(path.join(value.project, "plugins", "beta"), { recursive: true });
  fs.mkdirSync(path.join(value.project, "plugins", "alpha"), { recursive: true });
  fs.writeFileSync(path.join(value.project, "ucodebench", "report.py"), 'def line():\n    return "items=1"\n', "utf8");
  fs.writeFileSync(path.join(value.project, "plugins", "beta", "config.py"), 'PLUGIN_NAME = "beta"\nTIMEOUT_SECONDS = 7\n', "utf8");
  fs.writeFileSync(path.join(value.project, "plugins", "alpha", "config.py"), 'PLUGIN_NAME = "alpha"\nTIMEOUT_SECONDS = 3\n', "utf8");
  fs.writeFileSync(path.join(value.project, "README.md"), "# Fixture\n", "utf8");
  let coderCalls = 0;
  value.coder.generate = async (payload) => {
    coderCalls += 1;
    throw new Error(`deterministic replacements must not call the coder for ${payload.requested_tool.arguments.path}`);
  };

  const ticket = value.service.createTicket(
    "创建一个三文件待审批批次：把 ucodebench/report.py 中输出前缀从 items= 改为 count=；" +
    "把准确路径 plugins/beta/config.py 中 TIMEOUT_SECONDS 从 7 改为 9，不能修改 plugins/alpha/config.py；" +
    "在 README.md 末尾增加一行 `Report output uses count=N.`。三个文件必须放在同一个审批批次，未批准前不要写入。",
  );
  const result = await value.service.plan(ticket.id);

  assert.equal(coderCalls, 0);
  assert.equal(result.route, "explicit_batch_edit");
  assert.deepEqual(result.effect.changes.map((change) => change.path), [
    "ucodebench/report.py",
    "plugins/beta/config.py",
    "README.md",
  ]);
  assert.match(result.effect.changes[0].content, /count=1/);
  assert.doesNotMatch(result.effect.changes[0].content, /Report output uses/);
  assert.match(result.effect.changes[1].content, /TIMEOUT_SECONDS = 9/);
  assert.match(result.effect.changes[2].content, /Report output uses count=N\./);
  assert.match(fs.readFileSync(path.join(value.project, "ucodebench", "report.py"), "utf8"), /items=1/);
  assert.match(fs.readFileSync(path.join(value.project, "plugins", "alpha", "config.py"), "utf8"), /TIMEOUT_SECONDS = 3/);
});

test("explicit mixed create modify delete bypasses an invalid controller and remains reversible", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const modifyPath = path.join(value.project, "mixed_modify.py");
  const deletePath = path.join(value.project, "mixed_delete.txt");
  const createPath = path.join(value.project, "mixed_created.py");
  const modifyBefore = 'def current_value():\n    return "old-value"\n';
  const deleteBefore = "delete-after-approval\n";
  fs.writeFileSync(modifyPath, modifyBefore, "utf8");
  fs.writeFileSync(deletePath, deleteBefore, "utf8");
  let controllerCalls = 0;
  let createAttempts = 0;
  value.service.model.generate = async () => {
    controllerCalls += 1;
    return { type: "tool_request", content: "", tool: { name: "read_file", arguments: { path: "read_file" } } };
  };
  value.coder.generate = async (payload) => {
    const pathname = payload.requested_tool.arguments.path;
    if (pathname === "mixed_created.py") createAttempts += 1;
    const content = pathname === "mixed_modify.py"
      ? 'def current_value():\n    return "new-value"\n'
      : createAttempts === 1
        ? 'def created_value():\n    return "new-value"\n'
        : 'def created_value():\n    return "created-value"\n';
    return { type: "tool_request", content: "", tool: { name: "write_file", arguments: { path: pathname, content } } };
  };

  const ticket = value.service.createTicket("把 mixed_modify.py 的返回值改成 new-value；新建 mixed_created.py 并实现 created_value()，返回 created-value；删除 mixed_delete.txt");
  const result = await value.service.plan(ticket.id);
  assert.equal(controllerCalls, 0);
  assert.equal(createAttempts, 2);
  assert.deepEqual(result.effect.changes.map((change) => [change.path, change.operation]), [
    ["mixed_modify.py", "modify"],
    ["mixed_created.py", "create"],
    ["mixed_delete.txt", "delete"],
  ]);
  assert.deepEqual(ticket.execution.evidence.map((item) => item.sourcePaths[0]), ["mixed_modify.py", "mixed_delete.txt"]);
  assert.equal(fs.readFileSync(modifyPath, "utf8"), modifyBefore);
  assert.equal(fs.existsSync(createPath), false);
  assert.equal(fs.readFileSync(deletePath, "utf8"), deleteBefore);
  value.service.approve(ticket.id, result.effect.id, true);
  assert.match(fs.readFileSync(modifyPath, "utf8"), /new-value/);
  assert.match(fs.readFileSync(createPath, "utf8"), /created-value/);
  assert.equal(fs.existsSync(deletePath), false);
  value.service.rollback(ticket.id, result.effect.id);
  assert.equal(fs.readFileSync(modifyPath, "utf8"), modifyBefore);
  assert.equal(fs.existsSync(createPath), false);
  assert.equal(fs.readFileSync(deletePath, "utf8"), deleteBefore);
});

test("approve-selected applies and rolls back only the chosen batch files", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const readmeBefore = fs.readFileSync(path.join(value.project, "README.md"), "utf8");
  const headerPath = path.join(value.project, "docs", "thesis_header.md");
  const headerBefore = fs.readFileSync(headerPath, "utf8");
  const ticket = value.service.createTicket("选择性批准文档修改");
  const proposal = await value.service.callTool(ticket.id, "write_files", { changes: [
    { path: "README.md", operation: "modify", content: `${readmeBefore}\nSelected\n` },
    { path: "docs/thesis_header.md", operation: "modify", content: `${headerBefore}\nNot selected\n` },
  ] });

  const result = value.service.approve(ticket.id, proposal.effect.id, true, ["README.md"]);
  assert.deepEqual(result.paths, ["README.md"]);
  assert.match(fs.readFileSync(path.join(value.project, "README.md"), "utf8"), /Selected/);
  assert.equal(fs.readFileSync(headerPath, "utf8"), headerBefore);
  value.service.rollback(ticket.id, proposal.effect.id);
  assert.equal(fs.readFileSync(path.join(value.project, "README.md"), "utf8"), readmeBefore);
  assert.equal(fs.readFileSync(headerPath, "utf8"), headerBefore);
});

test("saves generated code from the previous assistant turn only after approval", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const ticket = value.service.createTicket("帮我做一个背包问题的 Python 代码");
  value.service.hydrateTicket(ticket.id, [
    {
      role: "assistant",
      content: [
        "下面是一份可直接运行的实现：",
        "```python",
        "def knapsack_0_1(capacity, weights, values):",
        "    return max(values, default=0) if capacity else 0",
        "```",
      ].join("\n"),
    },
  ]);
  value.service.continueTicket(ticket.id, "保存成.py文件到当前目录");

  const proposed = await value.service.plan(ticket.id);
  assert.equal(proposed.route, "save_generated_file");
  assert.equal(proposed.effect.status, "pending");
  assert.equal(proposed.effect.path, "knapsack.py");
  assert.match(proposed.effect.content, /def knapsack_0_1/);
  assert.equal(fs.existsSync(path.join(value.project, "knapsack.py")), false);

  value.service.approve(ticket.id, proposed.effect.id, true);
  const completed = await value.service.plan(ticket.id);
  assert.equal(completed.type, "final");
  assert.match(completed.content, /已应用/);
  assert.match(fs.readFileSync(path.join(value.project, "knapsack.py"), "utf8"), /def knapsack_0_1/);
});

test("honours an explicit filename when saving generated code", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const ticket = value.service.createTicket("帮我写一个背包算法");
  value.service.hydrateTicket(ticket.id, [
    { role: "assistant", content: "```python\ndef solve_knapsack():\n    return []\n```" },
  ]);
  value.service.continueTicket(ticket.id, "把它保存为 backpack_solver.py");

  const proposed = await value.service.plan(ticket.id);
  assert.equal(proposed.effect.path, "backpack_solver.py");
  assert.equal(fs.existsSync(path.join(value.project, "backpack_solver.py")), false);
});

test("cancelling generation leaves the Agent ticket stopped instead of failed", async (t) => {
  const value = fixture();
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  value.service.coder.generateStream = async (_payload, { signal }) => {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(resolve, 1_000);
      signal.addEventListener("abort", () => {
        clearTimeout(timer);
        const error = new Error("cancelled");
        error.code = "cancelled";
        reject(error);
      }, { once: true });
    });
    return { type: "final", content: "too late" };
  };
  const ticket = value.service.createTicket("请详细介绍如何培养良好的学习习惯，并给出具体建议");
  const controller = new AbortController();
  const pending = value.service.plan(ticket.id, { signal: controller.signal, onEvent() {} });
  setTimeout(() => controller.abort(), 10);
  await assert.rejects(pending, /cancelled/);
  assert.equal(value.service.ticket(ticket.id).state, "stopped");
  assert.equal(value.service.ticket(ticket.id).reports.at(-1).kind, "dropped");
});
