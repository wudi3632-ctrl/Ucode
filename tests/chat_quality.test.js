import test from "node:test";
import assert from "node:assert/strict";
import { enforceOneSentence, exactReplyConstraint, exactReplyMatches, oneSentenceConstraint } from "../agent/executor/chat_quality.js";

test("extracts bounded Chinese and English exact-reply constraints", () => {
  assert.deepEqual(
    exactReplyConstraint("通知后台样本：只回答“后台完成”，不要读取或修改项目文件。"),
    { target: "后台完成" },
  );
  assert.deepEqual(exactReplyConstraint("Reply with exactly build_ok."), { target: "build_ok" });
  assert.equal(exactReplyConstraint("请解释为什么 build_ok 是合理的"), null);
});

test("exact-reply comparison accepts only the complete requested string", () => {
  const constraint = { target: "progressive_startup_ok" };
  assert.equal(exactReplyMatches("progressive_startup_ok", constraint), true);
  assert.equal(exactReplyMatches("True", constraint), false);
  assert.equal(exactReplyMatches("`progressive_startup_ok`", constraint), false);
  assert.equal(exactReplyMatches("progressive_startup_ok。", constraint), false);
});

test("Chinese punctuation after an exact-reply instruction is not part of the target", () => {
  assert.deepEqual(exactReplyConstraint("只回答：你好"), { target: "你好" });
});

test("one-sentence requests are detected and multi-sentence output is normalized", () => {
  assert.equal(oneSentenceConstraint("用一句话解释什么是动态规划，不要写代码。"), true);
  assert.equal(oneSentenceConstraint("Explain dynamic programming in one sentence."), true);
  assert.equal(oneSentenceConstraint("解释动态规划"), false);
  assert.equal(
    enforceOneSentence("动态规划将问题拆成重叠子问题。它保存子问题结果以避免重复计算。"),
    "动态规划将问题拆成重叠子问题，它保存子问题结果以避免重复计算。",
  );
});
