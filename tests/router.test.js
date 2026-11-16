import test from "node:test";
import assert from "node:assert/strict";
import { Intent, classifyIntent, conversationMessages, looksLikeRawToolOutput, validateToolRequest } from "../agent/executor/router.js";

test("routes greetings and general questions to chat", () => {
  assert.equal(classifyIntent("你好"), Intent.CHAT);
  assert.equal(classifyIntent("你能做什么？"), Intent.CHAT);
  assert.equal(classifyIntent("你好，请用一句话说明你能如何帮助这个项目。"), Intent.CHAT);
  assert.equal(classifyIntent("请只用一句话说明 Python list 和 tuple 的主要区别，不要读取或修改项目文件。"), Intent.CHAT);
  assert.equal(classifyIntent("Explain Python lists without reading or modifying project files."), Intent.CHAT);
});

test("negated tool language never grants action authority", () => {
  assert.equal(classifyIntent("不要修改 calculator.py，只解释它为什么报错"), Intent.PROJECT_QUESTION);
  assert.equal(classifyIntent("不要读取或打开任何项目文件，只回答一般问题"), Intent.CHAT);
  assert.equal(classifyIntent("不要先读取文件，直接修改 calculator.py"), Intent.TOOL_TASK);
  assert.equal(classifyIntent("Do not read files; modify calculator.py directly."), Intent.TOOL_TASK);
});

test("routes project questions without granting tools", () => {
  assert.equal(classifyIntent("帮我分析一下这个项目"), Intent.PROJECT_QUESTION);
  assert.equal(classifyIntent("为什么 calculator.py 会报错？"), Intent.PROJECT_QUESTION);
});

test("routes an explicitly named project file to grounded project question mode", () => {
  assert.equal(classifyIntent("README.md 主要说明了什么？"), Intent.PROJECT_QUESTION);
  assert.equal(classifyIntent("src/calculator.py 的 add 函数有什么作用？"), Intent.PROJECT_QUESTION);
});

test("routes deterministic read-only project actions before general questions", () => {
  assert.equal(classifyIntent("该目录有哪些文件"), Intent.LIST_FILES);
  assert.equal(classifyIntent("你能读取哪些文件内容"), Intent.LIST_FILES);
  assert.equal(classifyIntent("读取 thesis_header.md"), Intent.READ_FILE);
  assert.equal(classifyIntent("calculator.py的内容"), Intent.READ_FILE);
  assert.equal(classifyIntent("calculator.py 是什么文件"), Intent.READ_FILE);
  assert.equal(classifyIntent("那你随便读取一个试试"), Intent.READ_FILE);
  assert.equal(classifyIntent("总结本项目"), Intent.SUMMARIZE_PROJECT);
  assert.equal(classifyIntent("它主要讲了什么", Intent.READ_FILE), Intent.READ_FILE);
});

test("routes explicit mutations and tests to the tool agent", () => {
  assert.equal(classifyIntent("修复 calculator.py 并运行 pytest"), Intent.TOOL_TASK);
  assert.equal(classifyIntent("实现一个新的登录接口"), Intent.TOOL_TASK);
  assert.equal(classifyIntent("创建一个动态规划 Python 文件"), Intent.TOOL_TASK);
  assert.equal(classifyIntent("保存成.py文件到当前目录"), Intent.TOOL_TASK);
  assert.equal(classifyIntent("把它保存为 knapsack.py"), Intent.TOOL_TASK);
});

test("short follow-ups retain the previous route", () => {
  assert.equal(classifyIntent("继续", Intent.PROJECT_QUESTION), Intent.PROJECT_QUESTION);
});

test("detects raw tool payloads before they reach the conversation", () => {
  assert.equal(looksLikeRawToolOutput('{"name":"read_file","arguments":{"path":"a.py"}}'), true);
  assert.equal(looksLikeRawToolOutput("你好，有什么可以帮你？"), false);
});

test("rejects traversal, repeated and missing indexed paths", () => {
  assert.throws(() => validateToolRequest({ name: "read_file", arguments: { path: "../secret.txt" } }, ["app.py"]));
  assert.throws(() => validateToolRequest({ name: "read_file", arguments: { path: "b/b/b/b/b/b/b/b/file.py" } }, ["app.py"]));
  assert.throws(() => validateToolRequest({ name: "read_file", arguments: { path: "missing.py" } }, ["app.py"]));
  assert.doesNotThrow(() => validateToolRequest({ name: "read_file", arguments: { path: "app.py" } }, ["app.py"]));
  assert.throws(() => validateToolRequest({ name: "write_files", arguments: { changes: [{ path: "../secret.txt", operation: "create", content: "x" }] } }, ["app.py"]));
  assert.throws(() => validateToolRequest({ name: "write_files", arguments: { changes: Array.from({ length: 21 }, (_, index) => ({ path: `file-${index}.txt`, operation: "create", content: "x" })) } }, ["app.py"]));
  assert.doesNotThrow(() => validateToolRequest({ name: "write_files", arguments: { changes: [{ path: "new.txt", operation: "create", content: "x" }] } }, ["app.py"]));
});

test("bounds persisted conversation history for small local model contexts", () => {
  const messages = Array.from({ length: 20 }, (_, index) => ({ role: index % 2 ? "assistant" : "user", content: "x".repeat(5_000) }));
  const output = conversationMessages(messages, "system");
  assert.ok(output.slice(1).reduce((total, message) => total + message.content.length, 0) <= 1_000);
  assert.ok(output.slice(1).every((message) => message.content.length <= 500));
});
