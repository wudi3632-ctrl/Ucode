import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { ProjectTools } from "../agent/tools/tools.js";
import { StatelessExecutor } from "../agent/executor/stateless.js";

function fixture(mode, text) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ucode-stateless-"));
  fs.writeFileSync(path.join(root, "README.md"), "# UCode\n\nPortable desktop agent.");
  const events = [];
  const calls = [];
  const coder = {
    async generate(payload) {
      calls.push(payload);
      if (payload.mode === "chat") return { type: "final", content: "真实回答" };
      return { type: "tool_request", content: "", tool: { name: "write_file", arguments: { path: "README.md", content: "# Changed\n\nAwaiting approval." } } };
    },
    async generateStream(payload, { onEvent }) {
      calls.push(payload);
      onEvent({ type: "text_delta", delta: "真实" });
      onEvent({ type: "thought", reasoning: "must not escape" });
      onEvent({ type: "text_delta", delta: "回答" });
      return { type: "final", content: "真实回答" };
    },
  };
  const executor = new StatelessExecutor({
    tools: new ProjectTools({ timeoutMs: 1_000, maxOutputBytes: 20_000 }),
    coder: () => coder,
    controller: { async generate() { return { type: "tool_request", content: "", tool: { name: "write_file", arguments: { path: "README.md", content: "placeholder" } } }; } },
  });
  const request = {
    version: "ucode.executor.v1",
    requestId: "request",
    turnId: "turn",
    threadKey: "desktop:conversation",
    mode,
    assignment: null,
    mandate: null,
    inputs: [{ id: "input", kind: "trigger", triggerId: "trigger", text, human: true }],
    history: [{ role: "user", content: "previous" }],
    workspace: { canonicalRoot: root, fingerprint: "fingerprint", files: [{ path: "README.md", sizeBytes: 32, contentDigest: null }] },
    attachmentToken: null,
    attachments: [],
    metadata: {},
  };
  return { root, events, calls, executor, request };
}

test("stateless chat streams only text deltas and persists no Node ticket", async (t) => {
  const value = fixture("chat", "你好");
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });
  assert.equal(content, "真实回答");
  assert.deepEqual(value.events, [{ type: "text_delta", delta: "真实" }, { type: "text_delta", delta: "回答" }]);
  assert.equal(value.calls[0].messages.at(-1).content, "你好");
});

test("exact-reply chat is fulfilled deterministically without calling the model", async (t) => {
  const value = fixture("chat", "只回答 progressive_startup_ok，不要读取或修改项目文件。");
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const coder = value.executor.coder();
  let attempt = 0;
  coder.generate = async (payload) => {
    value.calls.push(payload);
    attempt += 1;
    return { type: "final", content: attempt === 1 ? "True" : "progressive_startup_ok" };
  };

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });

  assert.equal(content, "progressive_startup_ok");
  assert.equal(attempt, 0);
  assert.deepEqual(value.events, [{ type: "text_delta", delta: "progressive_startup_ok" }]);
  assert.equal(value.calls.length, 0);
});

test("exact-reply chat remains available while the model is unavailable", async (t) => {
  const value = fixture("chat", "Reply with exactly build_ok.");
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  value.executor.coder = () => null;

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });
  assert.equal(content, "build_ok");
  assert.deepEqual(value.events, [{ type: "text_delta", delta: "build_ok" }]);
});

test("a simple question about the previous exact-reply instruction uses real history", async (t) => {
  const value = fixture("chat", "我上一句要求你只回答什么？");
  value.request.history = [
    { role: "user", content: "只回答：你好" },
    { role: "assistant", content: "你好" },
  ];
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });

  assert.equal(content, "你好");
  assert.equal(value.calls.length, 0);
  assert.doesNotMatch(content, /Python/);
});

test("capability chat is answered by the model without requiring project evidence", async (t) => {
  const value = fixture("chat", "你好，请说明你能如何帮助这个项目。");
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const answer = await value.executor.execute(value.request);
  assert.equal(answer, "真实回答");
  assert.equal(value.calls.length, 1);
  assert.equal(value.calls[0].mode, "chat");
  assert.match(value.calls[0].messages.at(0).content, /不输出工具调用或 JSON/);
  assert.match(value.calls[0].messages.at(0).content, /不套用固定开头/);
  assert.doesNotMatch(value.calls[0].messages.at(0).content, /第一行必须先写时间复杂度/);
});

test("persisted task context is supplied to the model without granting authority", async (t) => {
  const value = fixture("chat", "继续");
  value.request.metadata.taskContext = {
    objective: "修复 src/app.py",
    targetFiles: ["src/app.py"],
    constraints: ["保持公开 API 不变"],
    verification: ["运行 pytest"],
    unresolvedQuestions: [],
  };
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  await value.executor.execute(value.request);

  const system = value.calls[0].messages[0].content;
  assert.match(system, /宿主持久化的任务上下文/);
  assert.match(system, /目标：修复 src\/app\.py/);
  assert.match(system, /保持公开 API 不变/);
  assert.equal(value.request.mandate, null);
});

test("project read uses only the Rust-supplied index and emits factual activity", async (t) => {
  const value = fixture("project_read", "解释 README.md");
  fs.writeFileSync(path.join(value.root, "secret.txt"), "must not be read");
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });
  assert.match(content, /真实回答/);
  assert.match(content, /README\.md/);
  assert.equal(value.events[0].type, "activity");
  assert.equal(value.events[0].evidence.path, "README.md");
  assert.ok(value.calls[0].workspace.files.every((file) => file.path !== "secret.txt"));
  assert.deepEqual(value.calls[0].workspace.files, []);
  assert.match(value.calls[0].messages.at(-1).content, /桌面宿主刚读取的真实文件内容/);
  assert.match(value.calls[0].messages.at(-1).content, /Portable desktop agent/);
  assert.match(value.calls[0].messages.at(-1).content, /问题：解释 README\.md/);
});

test("project read retries once without streaming after a tool-shaped answer", async (t) => {
  const value = fixture("project_read", "读取准确路径 README.md。告诉我项目名称。");
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const coder = value.executor.coder();
  coder.generateStream = async (payload) => {
    value.calls.push(payload);
    return { type: "tool_request", content: "", tool: { name: "read_file", arguments: { path: "README.md" } } };
  };
  coder.generate = async (payload) => {
    value.calls.push(payload);
    return { type: "final", content: "项目名称是 UCode。" };
  };

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });
  assert.match(content, /项目名称是 UCode/);
  assert.equal(value.calls.length, 2);
  assert.match(value.calls[1].messages[0].content, /上一次输出错误地生成了工具调用/);
  assert.doesNotMatch(value.calls[0].messages.at(-1).content, /读取准确路径/);
});

test("direct file-content questions return a fenced code block with real newlines", async (t) => {
  const value = fixture("project_read", "calculator.py的内容");
  fs.writeFileSync(path.join(value.root, "calculator.py"), 'def add(a, b):\n    """Return the sum of two numbers."""\n    return a + b\n');
  value.request.workspace.files.push({ path: "calculator.py", sizeBytes: 73, contentDigest: null });
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });

  assert.match(content, /^`calculator\.py` 的内容如下：\n\n```py\n/);
  assert.match(content, /def add\(a, b\):\n {4}"""Return the sum/);
  assert.doesNotMatch(content, /:\\n/);
  assert.equal(value.calls.length, 0);
});

test("what-file questions receive an evidence-based local description", async (t) => {
  const value = fixture("project_read", "calculator.py是什么文件");
  fs.writeFileSync(path.join(value.root, "calculator.py"), "def add(a, b):\n    return a + b\n");
  value.request.workspace.files.push({ path: "calculator.py", sizeBytes: 32, contentDigest: null });
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });

  assert.match(content, /Python 源文件/);
  assert.match(content, /`add`/);
  assert.match(content, /来源/);
  assert.doesNotMatch(content, /calculator\.py是文件。/);
  assert.equal(value.calls.length, 0);
});

test("project route without a selected file asks the model instead of returning a canned evidence fallback", async (t) => {
  const value = fixture("project_read", "写一个背包问题的 Python 代码");
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });
  assert.equal(content, "真实回答");
  assert.equal(value.calls.length, 1);
  assert.equal(value.calls[0].mode, "chat");
  assert.doesNotMatch(content, /没有读取到能支持回答的文件/);
});

test("project diagnosis without a file or error asks for evidence instead of completing nonsense", async (t) => {
  const value = fixture("project_read", "为什么这个项目会报错？");
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });

  assert.match(content, /提供报错文本|指定要检查的相对路径/);
  assert.equal(value.calls.length, 0);
  assert.equal(value.events.at(-1).type, "report");
  assert.equal(value.events.at(-1).state, "waiting");
});

test("an explicit missing file is rejected before the model can hallucinate content", async (t) => {
  const value = fixture("project_read", "missing.py的内容");
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });

  assert.equal(content, "项目索引中不存在 `missing.py`。请从项目文件列表中选择存在的相对路径。");
  assert.equal(value.calls.length, 0);
  assert.equal(value.events.at(-1).state, "waiting");
  assert.doesNotMatch(content, /\\n/);
});

test("a traversal path is rejected before invoking either local model", async (t) => {
  const value = fixture("project_read", "读取 ../secret.txt 并解释");
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });

  assert.match(content, /不能读取项目外路径/);
  assert.match(content, /\.\.\/secret\.txt/);
  assert.equal(value.calls.length, 0);
  assert.equal(value.events.at(-1).state, "waiting");
});

test("action path proposes a durable Effect payload without writing the file", async (t) => {
  const value = fixture("action", "修改 README.md");
  value.request.assignment = { ticketId: "ticket", assignmentId: "assignment" };
  value.request.mandate = {
    strength: "explicit",
    policyVersion: "1",
    projectId: "project",
    projectFingerprint: "fingerprint",
    capabilities: ["read", "propose_write"],
    triggerId: "trigger",
    threadKey: value.request.threadKey,
  };
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));
  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });
  assert.match(content, /等待审批/);
  const proposal = value.events.find((event) => event.type === "effect_proposal");
  assert.equal(proposal.kind, "write_file");
  assert.equal(proposal.payload.path, "README.md");
  assert.match(proposal.payloadDigest, /^[0-9a-f]{64}$/);
  assert.equal(value.events.at(-1).state, "waiting");
  assert.equal(fs.readFileSync(path.join(value.root, "README.md"), "utf8"), "# UCode\n\nPortable desktop agent.");
});

test("a create-file task repairs one unsafe controller path and pins the coder to the repaired path", async (t) => {
  const value = fixture("action", "请创建 dynamic_programming.py，包含动态规划示例");
  value.request.assignment = { ticketId: "ticket", assignmentId: "assignment" };
  value.request.mandate = {
    strength: "explicit",
    policyVersion: "1",
    projectId: "project",
    projectFingerprint: "fingerprint",
    capabilities: ["read", "propose_write"],
    triggerId: "trigger",
    threadKey: value.request.threadKey,
  };
  value.executor.controller.generate = async () => ({
    type: "tool_request",
    content: "",
    tool: { name: "write_file", arguments: { path: "b/../b/../dynamic_programming.py", content: "placeholder" } },
  });
  value.executor.coder().generate = async (payload) => {
    value.calls.push(payload);
    return {
      type: "tool_request",
      content: "",
      tool: { name: "write_file", arguments: { path: "C:/unsafe/output.py", content: "def solve():\n    return 1\n" } },
    };
  };
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });

  assert.match(content, /等待审批/);
  const proposal = value.events.find((event) => event.type === "effect_proposal");
  assert.equal(proposal.payload.path, "dynamic_programming.py");
  assert.equal(proposal.payload.content, "def solve():\n    return 1\n");
  assert.ok(value.events.some((event) => event.kind === "path_repair"));
  assert.equal(fs.existsSync(path.join(value.root, "dynamic_programming.py")), false);
});

test("an unnamed dynamic-programming file bypasses a confused controller and still requires approval", async (t) => {
  const value = fixture("action", "帮我写一个动态规划的python代码，并保存成py文件");
  value.request.assignment = { ticketId: "ticket", assignmentId: "assignment" };
  value.request.mandate = {
    strength: "explicit",
    policyVersion: "1",
    projectId: "project",
    projectFingerprint: "fingerprint",
    capabilities: ["read", "propose_write"],
    triggerId: "trigger",
    threadKey: value.request.threadKey,
  };
  let controllerCalls = 0;
  value.executor.controller.generate = async () => {
    controllerCalls += 1;
    return { type: "tool_request", content: "", tool: { name: "read_file", arguments: { path: "missing.py" } } };
  };
  value.executor.coder().generate = async (payload) => {
    value.calls.push(payload);
    return {
      type: "final",
      content: "```python\ndef knapsack(values):\n    return max(values, default=0)\n```",
    };
  };
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });

  assert.match(content, /dynamic_programming\.py/);
  assert.equal(controllerCalls, 0);
  assert.equal(value.calls[0].requested_tool.arguments.path, "dynamic_programming.py");
  assert.match(value.calls[0].messages[0].content, /目标路径已经由桌面宿主安全锁定/);
  assert.doesNotMatch(value.calls[0].messages[0].content, /只能返回给定工具调用/);
  const proposal = value.events.find((event) => event.type === "effect_proposal");
  assert.equal(proposal.payload.path, "dynamic_programming.py");
  assert.ok(value.events.some((event) => event.kind === "path_inference"));
  assert.equal(fs.existsSync(path.join(value.root, "dynamic_programming.py")), false);
});

test("action path cannot report a mutation complete without an Effect", async (t) => {
  const value = fixture("action", "修复这个项目中的错误");
  value.request.assignment = { ticketId: "ticket", assignmentId: "assignment" };
  value.request.mandate = {
    strength: "explicit",
    policyVersion: "1",
    projectId: "project",
    projectFingerprint: "fingerprint",
    capabilities: ["read", "propose_write"],
    triggerId: "trigger",
    threadKey: value.request.threadKey,
  };
  value.executor.controller.generate = async () => ({ type: "final", content: "已经修复完成。" });
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });
  assert.match(content, /还没有生成可审查的修改 Diff/);
  assert.doesNotMatch(content, /已经修复完成/);
  assert.equal(value.events.at(-1).type, "report");
  assert.equal(value.events.at(-1).state, "waiting");
});

test("action path reports a warming Controller without disabling narrower work", async (t) => {
  const value = fixture("action", "分析整个项目并完成跨模块重构");
  value.request.assignment = { ticketId: "ticket", assignmentId: "assignment" };
  value.request.mandate = {
    strength: "explicit",
    policyVersion: "1",
    projectId: "project",
    projectFingerprint: "fingerprint",
    capabilities: ["read", "propose_write"],
    triggerId: "trigger",
    threadKey: value.request.threadKey,
  };
  value.executor.controller.generate = async () => { throw Object.assign(new Error("connection refused"), { code: "model_unavailable" }); };
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  await assert.rejects(
    () => value.executor.execute(value.request),
    (error) => error.code === "controller_unavailable"
      && /普通对话和明确指定文件的任务仍可继续/.test(error.message),
  );
});

test("action read evidence alone cannot prove a requested mutation completed", async (t) => {
  const value = fixture("action", "修复这个项目中的错误");
  value.request.assignment = { ticketId: "ticket", assignmentId: "assignment" };
  value.request.mandate = {
    strength: "explicit",
    policyVersion: "1",
    projectId: "project",
    projectFingerprint: "fingerprint",
    capabilities: ["read", "propose_write"],
    triggerId: "trigger",
    threadKey: value.request.threadKey,
  };
  let turn = 0;
  value.executor.controller.generate = async () => {
    turn += 1;
    return turn === 1
      ? { type: "tool_request", content: "", tool: { name: "read_file", arguments: { path: "README.md" } } }
      : { type: "final", content: "修改完成。" };
  };
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });
  assert.match(content, /还没有生成可审查的修改 Diff/);
  assert.ok(value.events.some((event) => event.type === "activity" && event.kind === "read_file"));
  assert.equal(value.events.at(-1).state, "waiting");
});

test("pytest-only action completes only after a real successful test result", async (t) => {
  const value = fixture("action", "运行 pytest");
  value.request.assignment = { ticketId: "ticket", assignmentId: "assignment" };
  value.request.mandate = {
    strength: "explicit",
    policyVersion: "1",
    projectId: "project",
    projectFingerprint: "fingerprint",
    capabilities: ["test"],
    triggerId: "trigger",
    threadKey: value.request.threadKey,
  };
  value.executor.tools.pytest = async () => ({ exitCode: 0, signal: null, stdout: "2 passed", stderr: "" });
  let turn = 0;
  value.executor.controller.generate = async () => {
    turn += 1;
    return turn === 1
      ? { type: "tool_request", content: "", tool: { name: "pytest", arguments: { paths: [] } } }
      : { type: "final", content: "测试通过。" };
  };
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });
  assert.equal(content, "测试通过。");
  assert.equal(value.events.find((event) => event.type === "activity" && event.kind === "pytest").evidence.exitCode, 0);
  assert.equal(value.events.at(-1).state, "ready");
});

test("bounded diagnosis runs pytest at most once per action turn", async (t) => {
  const value = fixture("action", "反复运行 pytest");
  value.request.assignment = { ticketId: "ticket", assignmentId: "assignment" };
  value.request.mandate = {
    strength: "explicit",
    policyVersion: "1",
    projectId: "project",
    projectFingerprint: "fingerprint",
    capabilities: ["test"],
    triggerId: "trigger",
    threadKey: value.request.threadKey,
  };
  let executions = 0;
  value.executor.tools.pytest = async () => {
    executions += 1;
    return { exitCode: 1, signal: null, stdout: "1 failed", stderr: "" };
  };
  value.executor.controller.generate = async () => ({
    type: "tool_request",
    content: "",
    tool: { name: "pytest", arguments: { paths: [] } },
  });
  t.after(() => fs.rmSync(value.root, { recursive: true, force: true }));

  const content = await value.executor.execute(value.request, { onEvent: (event) => value.events.push(event) });

  assert.equal(executions, 1);
  assert.match(content, /测试上限（1 次）/);
  assert.equal(value.events[0].kind, "budget");
  assert.equal(value.events.at(-1).state, "waiting");
});
