import path from "node:path";
import { AppError } from "../shared/errors.js";

export const Intent = Object.freeze({
  CHAT: "chat",
  LIST_FILES: "list_files",
  READ_FILE: "read_file",
  SUMMARIZE_PROJECT: "summarize_project",
  PROJECT_QUESTION: "project_question",
  TOOL_TASK: "tool_task",
});

const TOOL_TASK = /(修复|修改|改成|实现|新增|添加|增加|删除|重构|写入|创建|新建|保存为|保存成|保存到|存为|存成|写到|落盘|运行.{0,4}(测试|pytest)|执行.{0,4}(测试|pytest)|fix|modify|implement|refactor|save\s+(?:as|to)|write|run\s+(the\s+)?tests?)/i;
const LIST_FILES = /(有哪些文件|文件列表|目录结构|列出.{0,6}(文件|目录)|查看.{0,4}目录|能.{0,4}(读取|查看|打开).{0,6}(哪些|什么).{0,4}文件|what files|file list|list files)/i;
const SUMMARIZE_PROJECT = /(总结|概括|介绍|梳理).{0,8}(项目|仓库|目录)|(项目|仓库).{0,8}(总结|概括|介绍)|summari[sz]e.{0,8}(project|repository)/i;
const READ_FILE = /(读取|打开|查看|看看|读一下).{0,24}(文件|内容|源码|文档|\.\w{1,8}\b)|随便.{0,8}(读取|读|打开|看看)|任意.{0,8}(读取|读|打开)|read.{0,20}(file|source|document)/i;
const PROJECT_QUESTION = /(项目|代码|源码|文件|函数|方法|类|模块|报错|错误|异常|堆栈|测试|依赖|接口|仓库|目录|解释.{0,8}(代码|文件|函数)|分析.{0,8}(项目|代码|文件)|project|code|file|function|class|error|exception|stack|repository)/i;
const EXPLICIT_FILE_REFERENCE = /(?:^|[\s`'"“”‘’（(、，和与及])(?:[\w.@+-]+\/)*[\w.@+-]+\.[A-Za-z0-9]{1,12}(?=$|的|[\s`'"“”‘’）),，。！？!?：:；;、和与及])/u;
const CONTINUATION = /^(继续|接着|然后呢|为什么|详细一点|再说说|continue|why|more)[？?。.！!\s]*$/i;
const READ_CONTINUATION = /^(它|这个文件|刚才的文件).{0,12}(讲|内容|作用|做什么|解释|总结)|^(详细说说|展开说说)[？?。.！!\s]*$/i;
const CAPABILITY_CHAT = /(你|UCode).{0,6}(能|可以).{0,8}(怎么|如何|怎样)?\s*(帮助|协助)|你能做什么|有什么能力|支持什么/i;
const NEGATED_CHINESE_ACTION = /(?:不要|不用|无需|不需要|禁止|别|不应|不可|不能)\s*(?:再|先|自动|直接|擅自)?[^，,。；;！？!?\n]{0,16}?(?:读取|打开|查看|修复|修改|改成|实现|新增|添加|增加|删除|重构|写入|创建|新建|保存|运行|执行)/i;
const NEGATED_ENGLISH_ACTION = /(?:do\s+not|don't|never|without)\s+(?:automatically\s+|directly\s+|first\s+)?[^,.;!?\n]{0,24}?\b(?:read(?:ing)?|open(?:ing)?|inspect(?:ing)?|modif(?:y|ying)|edit(?:ing)?|writ(?:e|ing)|delet(?:e|ing)|creat(?:e|ing)|sav(?:e|ing)|run(?:ning)?|execut(?:e|ing))\b/i;

export function classifyIntent(request, previousIntent = null) {
  const text = String(request || "").trim();
  const actionableText = withoutNegatedActions(text);
  if (previousIntent === Intent.READ_FILE && READ_CONTINUATION.test(text)) return Intent.READ_FILE;
  if (previousIntent && CONTINUATION.test(text)) return previousIntent;
  if (TOOL_TASK.test(actionableText)) return Intent.TOOL_TASK;
  if (CAPABILITY_CHAT.test(text)) return Intent.CHAT;
  if (/(?:不要|不用|无需|不需要|别|do\s+not|don't|without).{0,12}(?:读取|打开|查看|read|open|inspect).{0,12}(?:项目|仓库|文件|project|repository|file)/i.test(text)
      && !EXPLICIT_FILE_REFERENCE.test(text)) return Intent.CHAT;
  if (LIST_FILES.test(actionableText)) return Intent.LIST_FILES;
  if (SUMMARIZE_PROJECT.test(actionableText)) return Intent.SUMMARIZE_PROJECT;
  if (READ_FILE.test(actionableText)) return Intent.READ_FILE;
  if (EXPLICIT_FILE_REFERENCE.test(actionableText) && /(?:完整)?(?:内容|源码|代码)|(?:是什么|什么|哪种)(?:类型的)?文件|what(?:'s| is)\s+in/i.test(actionableText)) return Intent.READ_FILE;
  if (EXPLICIT_FILE_REFERENCE.test(text) && /(解释|说明|分析|为什么|作用|做什么|explain|why|what)/i.test(text)) return Intent.PROJECT_QUESTION;
  if (EXPLICIT_FILE_REFERENCE.test(actionableText)) return Intent.PROJECT_QUESTION;
  if (PROJECT_QUESTION.test(actionableText)) return Intent.PROJECT_QUESTION;
  return Intent.CHAT;
}

function withoutNegatedActions(text) {
  return text.split(/([，,。；;！？!?\n]+)/).map((clause) => {
    const chinese = clause.search(NEGATED_CHINESE_ACTION);
    const english = clause.search(NEGATED_ENGLISH_ACTION);
    const indexes = [chinese, english].filter((index) => index >= 0);
    return indexes.length ? clause.slice(0, Math.min(...indexes)) : clause;
  }).join(" ").trim();
}

export function looksLikeRawToolOutput(content) {
  const text = String(content || "").trim();
  return /["'](?:name|type)["']\s*:\s*["'](?:read_file|search|write_file|write_files|diff|pytest)["']/i.test(text)
    || /<tool_call>/i.test(text);
}

export function validateToolRequest(tool, availableFiles = []) {
  const allowed = new Set(["read_file", "search", "write_file", "write_files", "diff", "pytest"]);
  if (!tool || !allowed.has(tool.name) || !tool.arguments || typeof tool.arguments !== "object") {
    throw new AppError("model_protocol_error", "模型返回了无效的工具调用，已停止执行。", 422);
  }
  const paths = tool.name === "write_files"
    ? (Array.isArray(tool.arguments.changes) ? tool.arguments.changes.map((change) => change?.path) : [])
    : tool.name === "pytest"
    ? (Array.isArray(tool.arguments.paths) ? tool.arguments.paths : [])
    : (typeof tool.arguments.path === "string" ? [tool.arguments.path] : []);
  for (const value of paths) validateRelativePath(value, tool.name, availableFiles);
  if (tool.name === "write_files" && (!Array.isArray(tool.arguments.changes) || tool.arguments.changes.length < 1 || tool.arguments.changes.length > 20)) {
    throw new AppError("model_protocol_error", "模型返回的文件批次为空或超过 20 个文件，已停止执行。", 422);
  }
  return tool;
}

function validateRelativePath(value, toolName, availableFiles) {
  if (typeof value !== "string" || !value.trim()) {
    throw new AppError("model_path_invalid", "模型没有提供有效的项目文件路径。", 422);
  }
  const normalizedInput = value.replaceAll("\\", "/");
  const segments = normalizedInput.split("/").filter(Boolean);
  const repeated = segments.length >= 8 && new Set(segments.slice(0, 8)).size <= 2;
  if (path.isAbsolute(value) || normalizedInput.startsWith("/") || normalizedInput.includes("://") || value.includes("\0") || segments.includes("..") || repeated || value.length > 260) {
    throw new AppError("model_path_invalid", "模型生成了越界或异常重复的文件路径，已安全停止。", 422);
  }
  if (["read_file", "diff", "pytest"].includes(toolName) && availableFiles.length && !availableFiles.includes(normalizedInput)) {
    throw new AppError("model_path_missing", `模型请求的文件不存在于项目索引中：${normalizedInput}`, 422);
  }
}

export function conversationMessages(messages, systemPrompt) {
  const candidates = messages
    .filter((message) => ["user", "assistant"].includes(message.role) && typeof message.content === "string" && message.content.trim())
    .slice(-12);
  const history = [];
  let remaining = 1_000;
  for (let index = candidates.length - 1; index >= 0 && remaining > 0; index -= 1) {
    const message = candidates[index];
    const content = message.content.trim().slice(0, Math.min(500, remaining));
    history.unshift({ role: message.role, content });
    remaining -= content.length;
  }
  return [{ role: "system", content: systemPrompt }, ...history];
}
