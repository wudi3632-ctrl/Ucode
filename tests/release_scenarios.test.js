import test from "node:test";
import assert from "node:assert/strict";
import { classifyIntent, Intent, validateToolRequest } from "../agent/executor/router.js";

const scenarios = [
  ["你好", null, Intent.CHAT],
  ["你能做什么？", null, Intent.CHAT],
  ["解释一下时间复杂度", null, Intent.CHAT],
  ["给我一个二分查找的思路", null, Intent.CHAT],
  ["不要读取项目，说明什么是依赖注入", null, Intent.CHAT],
  ["Do not open files; explain immutability.", null, Intent.CHAT],
  ["如何写出清晰的提交信息？", null, Intent.CHAT],
  ["只回答 OK，不执行任何操作", null, Intent.CHAT],

  ["列出项目文件", null, Intent.LIST_FILES],
  ["这个目录有哪些文件？", null, Intent.LIST_FILES],
  ["读取 README.md", null, Intent.READ_FILE],
  ["打开 src/main.rs 的源码", null, Intent.READ_FILE],
  ["总结这个项目", null, Intent.SUMMARIZE_PROJECT],
  ["概括当前仓库", null, Intent.SUMMARIZE_PROJECT],
  ["package.json 里的 scripts 有什么作用？", null, Intent.PROJECT_QUESTION],
  ["为什么这个项目测试失败？", null, Intent.PROJECT_QUESTION],
  ["解释 src/app.tsx", null, Intent.PROJECT_QUESTION],
  ["分析当前代码的入口", null, Intent.PROJECT_QUESTION],
  ["这个函数为什么会报错？", null, Intent.PROJECT_QUESTION],
  ["仓库使用了哪些依赖？", null, Intent.PROJECT_QUESTION],

  ["修复 calculator.py 并运行 pytest", null, Intent.TOOL_TASK],
  ["修改 src/app.tsx 的标题", null, Intent.TOOL_TASK],
  ["在 README.md 增加安装说明", null, Intent.TOOL_TASK],
  ["删除 deprecated.py", null, Intent.TOOL_TASK],
  ["重构 parser.rs，保持公开 API", null, Intent.TOOL_TASK],
  ["创建文件 healthcheck.py", null, Intent.TOOL_TASK],
  ["运行测试", null, Intent.TOOL_TASK],
  ["执行 pytest", null, Intent.TOOL_TASK],
  ["save as example.py", null, Intent.TOOL_TASK],
  ["fix the failing test in api.test.ts", null, Intent.TOOL_TASK],
  ["implement the TODO in src/lib.rs", null, Intent.TOOL_TASK],
  ["modify a.py and b.py, then run tests", null, Intent.TOOL_TASK],

  ["继续", Intent.TOOL_TASK, Intent.TOOL_TASK],
  ["为什么？", Intent.PROJECT_QUESTION, Intent.PROJECT_QUESTION],
  ["这个文件做什么？", Intent.READ_FILE, Intent.READ_FILE],
  ["详细说说", Intent.READ_FILE, Intent.READ_FILE],

  ["不要修改 calculator.py，只解释它", null, Intent.PROJECT_QUESTION],
  ["不要运行测试，解释测试结构", null, Intent.PROJECT_QUESTION],
  ["Do not modify src/app.ts; explain the file.", null, Intent.PROJECT_QUESTION],
  ["无需创建文件，介绍 Rust ownership", null, Intent.CHAT],
];

test(`release routing matrix covers ${scenarios.length} distinct user requests`, () => {
  const failures = [];
  for (const [request, previous, expected] of scenarios) {
    const actual = classifyIntent(request, previous);
    if (actual !== expected) failures.push({ request, previous, expected, actual });
  }
  assert.deepEqual(failures, []);
});

test("release safety matrix rejects traversal, absolute, repeated, remote, and missing paths", () => {
  const invalid = [
    "../secret.txt",
    "C:/Windows/System32/drivers/etc/hosts",
    "/etc/passwd",
    "https://example.com/file.py",
    "a/b/a/b/a/b/a/b/file.py",
    "missing.py",
  ];
  for (const path of invalid) {
    assert.throws(
      () => validateToolRequest({ name: "read_file", arguments: { path } }, ["src/app.py"]),
      (error) => ["model_path_invalid", "model_path_missing"].includes(error.code),
      path,
    );
  }
});

test("release safety matrix accepts only indexed project-relative read paths", () => {
  const result = validateToolRequest(
    { name: "read_file", arguments: { path: "src/app.py" } },
    ["src/app.py", "tests/test_app.py"],
  );
  assert.equal(result.arguments.path, "src/app.py");
});
