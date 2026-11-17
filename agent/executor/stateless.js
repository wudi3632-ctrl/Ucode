import path from "node:path";
import { createHash } from "node:crypto";
import { AppError } from "../shared/errors.js";
import { looksLikeRawToolOutput, validateToolRequest } from "./router.js";
import { validateExecutorRequest } from "./protocol.js";
import { resolveProjectRoot } from "../local_source/workspace.js";
import { TOOL_DEFINITIONS } from "../tools/definitions.js";
import { exactReplyConstraint } from "./chat_quality.js";

const CHAT_PROMPT = "你是 UCode 桌面应用中的本地编码助手。禁止读写项目文件只是本轮权限约束，不是取消回答。直接针对用户当前问题自然、简洁地使用中文回答，不套用固定开头，不复述权限说明，不输出工具调用或 JSON。只有用户明确要求代码时才给完整可运行代码；只有用户询问复杂度或复杂度确实有助于理解时才说明。";
const PROJECT_PROMPT = "你是 UCode 的项目问答助手。文件已经由桌面宿主读取完毕，你只做文本信息提取和解释，不再执行读取。只依据本轮真实文件内容，用完整、简洁的中文句子回答；先说明文件用途，再列出能够确认的关键内容。展示代码时必须使用带语言名的 Markdown 围栏，禁止把换行写成字面量 \\n。不猜测，不声称修改，不输出列表字面量、JSON、XML、函数调用、工具名称或任何虚构文件路径。来源路径由桌面宿主统一追加，你不需要重复。";
const PROJECT_RETRY_PROMPT = "上一次输出错误地生成了工具调用。本次文件内容已经在用户消息中，只能直接回答问题；禁止再次读取，禁止输出 JSON、代码形式的参数或任何工具调用。";
const READABLE = new Set([".md", ".txt", ".js", ".mjs", ".cjs", ".ts", ".tsx", ".jsx", ".py", ".rs", ".go", ".java", ".kt", ".c", ".h", ".cpp", ".hpp", ".cs", ".json", ".yaml", ".yml", ".toml", ".ini", ".cfg", ".xml", ".html", ".css", ".sql", ".sh", ".ps1", ".bat", ".cmd", ".log", ".csv"]);
const DIAGNOSIS_LIMITS = Object.freeze({ read_file: 10, search: 2, pytest: 1, write_batches: 1, stagnant_steps: 2 });

export class StatelessExecutor {
  constructor({ tools, coder, controller = null }) {
    this.tools = tools;
    this.coder = coder;
    this.controller = controller;
  }

  async execute(rawRequest, { signal, onEvent } = {}) {
    const request = validateExecutorRequest(rawRequest);
    if (request.mode === "action") return this.action(request, { signal, onEvent });
    if (request.mode === "chat") return this.chat(request, { signal, onEvent });
    return this.projectRead(request, { signal, onEvent });
  }

  async chat(request, options) {
    const query = request.inputs.filter((item) => item.human).map((item) => item.text).join("\n");
    const historyAnswer = answerFromConversationHistory(query, request.history);
    if (historyAnswer) {
      options.onEvent?.({ type: "text_delta", delta: historyAnswer });
      return historyAnswer;
    }
    const exact = exactReplyConstraint(query);
    if (exact) {
      const content = safeIdentityAnswer(request, exact.target);
      options.onEvent?.({ type: "text_delta", delta: content });
      return content;
    }
    const coder = this.coder();
    if (!coder) throw new AppError("chat_provider_unavailable", "对话模型尚未启动。", 503);
    const messages = modelMessages(request, CHAT_PROMPT);
    const result = await generate(coder, {
      task_id: request.turnId,
      messages,
      workspace: { root: request.workspace.canonicalRoot, files: attachmentFiles(request) },
      tools: [],
      mode: "chat",
    }, options);
    return safeIdentityAnswer(request, usableContent(result, "本地对话模型没有生成可用的文本回答。"));
  }

  async projectRead(request, options) {
    const root = resolveProjectRoot(request.workspace.canonicalRoot);
    const indexed = new Set(request.workspace.files.map((file) => file.path.replaceAll("\\", "/")));
    const actual = new Set(this.tools.listFiles(root));
    const allowed = [...indexed].filter((file) => actual.has(file));
    const query = request.inputs.map((input) => input.text).join("\n").trim();

    if (/(有哪些文件|文件列表|目录结构|列出.{0,6}(文件|目录)|what files|list files)/i.test(query)) {
      const shown = allowed.slice(0, 40);
      options.onEvent?.(activity("workspace-index", "list_files", "读取项目文件索引", "succeeded", { count: allowed.length, paths: shown }));
      return allowed.length
        ? `当前受限项目索引包含 ${allowed.length} 个文件，先显示前 ${shown.length} 个：\n\n${shown.map((file) => `- ${file}`).join("\n")}${allowed.length > shown.length ? `\n\n其余 ${allowed.length - shown.length} 个文件可通过文件名继续查找。` : ""}`
        : "当前受限项目索引中没有可读取文件。";
    }

    const selected = selectFiles(query, allowed);
    if (selected.status === "ambiguous") {
      return `找到多个同名文件，请指定相对路径：\n\n${selected.paths.map((file) => `- ${file}`).join("\n")}`;
    }
    if (!selected.paths.length) {
      const requested = extractRequestedPaths(query);
      const unsafe = requested.find((file) => unsafeRequestedPath(file)) || (hasPathTraversalIntent(query) ? ".." : null);
      if (unsafe) {
        const content = `为保护项目边界，不能读取项目外路径 \`${unsafe}\`。请指定项目索引中的相对路径。`;
        options.onEvent?.({ type: "report", state: "waiting", summary: content });
        return content;
      }
      const missing = requested.filter((file) => !allowed.some((allowedFile) => allowedFile.toLowerCase() === normalizeRequestedPath(file).toLowerCase()));
      if (missing.length) {
        const content = `项目索引中不存在 \`${normalizeRequestedPath(missing[0])}\`。请从项目文件列表中选择存在的相对路径。`;
        options.onEvent?.({ type: "report", state: "waiting", summary: content });
        return content;
      }
      if (requiresProjectEvidence(query)) {
        const content = "这个问题依赖项目中的真实报错信息，但我还没有可核验的文件或日志。请提供报错文本，或指定要检查的相对路径（例如 `src/main.py`）。";
        options.onEvent?.({ type: "report", state: "waiting", summary: content });
        return content;
      }
      const coder = this.coder();
      if (!coder) throw new AppError("chat_provider_unavailable", "对话模型尚未启动。", 503);
      const messages = modelMessages(
        request,
        "你是 UCode 桌面应用中的本地编码助手。当前没有读取任何项目文件。先判断用户的问题是否能作为通用问题直接回答：能回答就自然回答；只有问题明确依赖某个项目文件时，才询问一个具体的相对路径。不要声称读过项目，不要返回固定兜底模板，不要重复上一条澄清。",
      );
      const result = await generate(coder, {
        task_id: request.turnId,
        messages,
        workspace: { root, files: attachmentFiles(request) },
        tools: [],
        mode: "chat",
      }, options);
      return usableContent(result, "本地对话模型没有生成可用的文本回答。");
    }

    const files = [];
    for (const file of selected.paths) {
      const startedAt = new Date().toISOString();
      const result = this.tools.readFile(root, { path: file });
      files.push({ path: file, content: result.content.slice(0, selected.summary ? 1_600 : 12_000) });
      options.onEvent?.(activity(`read:${file}`, "read_file", `读取 ${file}`, "succeeded", {
        path: file,
        contentDigest: result.sha256,
        startedAt,
        finishedAt: new Date().toISOString(),
      }));
    }

    if (!selected.summary && /^(读取|打开|查看|看看|read)\b/i.test(query) && files.length === 1) {
      const extension = path.extname(files[0].path).slice(1);
      return `已读取 \`${files[0].path}\`。\n\n\`\`\`${extension}\n${files[0].content}\n\`\`\``;
    }

    if (!selected.summary && files.length === 1 && asksForFileContent(query)) {
      const extension = path.extname(files[0].path).slice(1) || "text";
      return `\`${files[0].path}\` 的内容如下：\n\n\`\`\`${extension}\n${files[0].content.trimEnd()}\n\`\`\``;
    }

    if (!selected.summary && files.length === 1 && asksWhatFileIs(query)) {
      return describeFile(files[0]);
    }

    const coder = this.coder();
    if (!coder) throw new AppError("chat_provider_unavailable", "对话模型尚未启动。", 503);
    const contextFiles = [...files, ...attachmentFiles(request)];
    const messages = projectMessages(request, contextFiles, PROJECT_PROMPT);
    const payload = {
      task_id: request.turnId,
      messages,
      // The evidence is already embedded in the final user message. Keeping
      // workspace empty prevents the small coder from mistaking a path index
      // for a request to call read_file again.
      workspace: { root, files: [] },
      tools: [],
      mode: "project_read",
    };
    const content = await trustedProjectContent(coder, payload, options);
    return `${content.trim()}\n\n来源\n${files.map((file) => `- \`${file.path}\``).join("\n")}`;
  }

  async action(request, options) {
    if (!this.controller) throw new AppError("controller_unavailable", "Controller model is not available", 503);
    const root = resolveProjectRoot(request.workspace.canonicalRoot);
    const actual = new Set(this.tools.listFiles(root));
    const allowed = request.workspace.files.map((file) => file.path.replaceAll("\\", "/")).filter((file) => actual.has(file));
    const capabilities = new Set(request.mandate.capabilities);
    const latestRequest = request.inputs.filter((input) => input.human).map((input) => input.text).join("\n");
    const messages = modelMessages(request, "你是 UCode 的受限任务控制器。只能返回给定工具调用；路径必须来自项目索引。单文件写入使用 write_file，多文件创建、修改或删除使用 write_files；都只能提出审批，不能声称已执行。不要输出私有思维过程。");
    const workspace = { root, files: actionWorkspace(this.tools, root, allowed, latestRequest) };
    const facts = [];
    const counts = { read_file: 0, search: 0, pytest: 0, write_batches: 0 };
    const evidenceKeys = new Set();
    let stagnantSteps = 0;
    let pathRepairAttempts = 0;
    options.onEvent?.(activity("diagnosis-budget", "budget", "已启用有界诊断", "succeeded", { limits: DIAGNOSIS_LIMITS }));

    const inferredCreatePath = implicitPythonCreatePath(latestRequest);
    if (inferredCreatePath) {
      requireCapability("write_file", capabilities);
      const coder = this.coder();
      if (!coder) throw new AppError("coder_unavailable", "代码模型尚未启动。", 503);
      const requestedTool = { name: "write_file", arguments: { path: inferredCreatePath } };
      const generated = await coder.generate({
        task_id: request.turnId,
        messages: writeContentMessages(request, inferredCreatePath),
        workspace,
        tools: [],
        mode: "write_content",
        requested_tool: requestedTool,
      }, { signal: options.signal });
      const tool = validateToolRequest(coderWriteProposal(generated, inferredCreatePath), allowed);
      const effect = this.tools.prepareWrite(root, tool.arguments);
      const payload = {
        changes: effect.changes,
        path: effect.path,
        content: effect.content,
        beforeDigest: effect.beforeDigest,
        afterDigest: effect.afterDigest,
      };
      const payloadDigest = createHash("sha256").update(JSON.stringify(payload)).digest("hex");
      options.onEvent?.(activity("implicit-create-path", "path_inference", `已将未命名的动态规划示例限制为 ${inferredCreatePath}`, "succeeded", { path: inferredCreatePath }));
      options.onEvent?.({ type: "effect_proposal", effectKey: `write-batch:${payloadDigest}`, kind: "write_file", capability: "propose_write", payloadDigest, payload });
      options.onEvent?.({ type: "report", state: "waiting", summary: "1 个文件变更等待审批" });
      return `已生成 \`${inferredCreatePath}\` 的修改提案，等待审批后才会写入。`;
    }

    for (let turn = 0; turn < 6; turn += 1) {
      let result;
      try {
        result = await this.controller.generate({
          task_id: request.turnId,
          messages,
          workspace,
          tools: TOOL_DEFINITIONS,
          mode: turn === 0 ? "plan" : "execute",
        }, { signal: options.signal });
      } catch (error) {
        if (error.code === "model_unavailable") {
          throw new AppError(
            "controller_unavailable",
            "复杂规划模型仍在启动或暂不可用。普通对话和明确指定文件的任务仍可继续；请稍后重试，或把任务缩小到具体文件与预期结果。",
            503,
          );
        }
        throw error;
      }
      if (result.type !== "tool_request") {
        const content = usableContent(result, "Controller did not produce a valid final response.");
        const verdict = gateActionCompletion(latestRequest, facts, content);
        options.onEvent?.({ type: "report", state: verdict.ready ? "ready" : "waiting", summary: verdict.content.slice(0, 500) });
        return verdict.content;
      }
      let validated;
      try {
        validated = validateToolRequest(result.tool, allowed);
      } catch (error) {
        const repaired = repairWriteToolPath(result.tool, latestRequest);
        if (repaired) {
          validated = validateToolRequest(repaired, allowed);
          options.onEvent?.(activity(`path-repair:${turn}`, "path_repair", `已将异常路径限制为 ${repaired.arguments.path}`, "succeeded", {
            rejectedPath: result.tool?.arguments?.path ?? null,
            repairedPath: repaired.arguments.path,
          }));
        } else if (["model_path_invalid", "model_path_missing"].includes(error?.code) && pathRepairAttempts < 1) {
          pathRepairAttempts += 1;
          const allowedHint = allowed.slice(0, 40).join("、") || "（当前项目没有现有文件）";
          messages.push({
            role: "user",
            content: `上一步路径被安全层拒绝：${error.message}\n请只使用项目相对路径，不得包含 ..、盘符、URL 或重复目录。现有文件：${allowedHint}。如果用户要求创建新的 Python 文件但没有命名，请使用 dynamic_programming.py。只返回一个修正后的工具调用。`,
          });
          options.onEvent?.(activity(`path-retry:${turn}`, "path_retry", "异常路径已拒绝，正在使用受限路径重试", "failed", {
            code: error.code,
            attempt: pathRepairAttempts,
          }));
          continue;
        } else {
          throw error;
        }
      }
      requireCapability(result.tool.name, capabilities);
      let tool = validated;
      if (tool.name === "write_file" && this.coder()) {
        const generated = await this.coder().generate({
          task_id: request.turnId,
          messages: writeContentMessages(request, tool.arguments.path),
          workspace,
          tools: [],
          mode: "write_content",
          requested_tool: tool,
        }, { signal: options.signal });
        // The Controller owns the approved path. The Coder only supplies file
        // content and must never redirect a write to another location.
        tool = validateToolRequest(coderWriteProposal(generated, tool.arguments.path), allowed);
      }
      messages.push({ role: "assistant", content: "", tool_calls: [tool] });

      if (["write_file", "write_files"].includes(tool.name)) {
        if (counts.write_batches >= DIAGNOSIS_LIMITS.write_batches) {
          return boundedStop(options, "本轮已达到 1 个修改批次上限。请先审查现有变更，再决定是否开始下一轮。");
        }
        counts.write_batches += 1;
        const effect = tool.name === "write_files"
          ? this.tools.prepareBatch(root, tool.arguments)
          : this.tools.prepareWrite(root, tool.arguments);
        const payload = {
          changes: effect.changes,
          path: effect.path,
          content: effect.content,
          beforeDigest: effect.beforeDigest,
          afterDigest: effect.afterDigest,
        };
        const payloadDigest = createHash("sha256").update(JSON.stringify(payload)).digest("hex");
        options.onEvent?.({
          type: "effect_proposal",
          effectKey: `write-batch:${payloadDigest}`,
          kind: tool.name,
          capability: "propose_write",
          payloadDigest,
          payload,
        });
        options.onEvent?.({ type: "report", state: "waiting", summary: `${effect.changes.length} 个文件变更等待审批` });
        return `已生成 ${effect.changes.length} 个文件的修改批次，等待审批后才会写入。`;
      }

      if (Object.hasOwn(DIAGNOSIS_LIMITS, tool.name) && counts[tool.name] >= DIAGNOSIS_LIMITS[tool.name]) {
        const labels = { read_file: "读取文件", search: "搜索", pytest: "测试" };
        return boundedStop(options, `本轮已达到${labels[tool.name] ?? tool.name}上限（${DIAGNOSIS_LIMITS[tool.name]} 次），已安全停止，避免无边界循环。`);
      }
      if (Object.hasOwn(counts, tool.name)) counts[tool.name] += 1;

      const startedAt = new Date().toISOString();
      const value = await executeReadOnlyTool(this.tools, root, tool);
      facts.push({ tool: tool.name, arguments: tool.arguments, value });
      const evidenceKey = createHash("sha256").update(JSON.stringify({ tool: tool.name, arguments: tool.arguments, value })).digest("hex");
      if (evidenceKeys.has(evidenceKey)) stagnantSteps += 1;
      else {
        evidenceKeys.add(evidenceKey);
        stagnantSteps = 0;
      }
      options.onEvent?.(activity(`tool:${turn}:${tool.name}`, tool.name, toolTitle(tool), "succeeded", {
        arguments: tool.arguments,
        startedAt,
        finishedAt: new Date().toISOString(),
        resultDigest: createHash("sha256").update(JSON.stringify(value)).digest("hex"),
        exitCode: Number.isInteger(value?.exitCode) ? value.exitCode : null,
      }));
      messages.push({ role: "tool", content: JSON.stringify(value) });
      if (stagnantSteps >= DIAGNOSIS_LIMITS.stagnant_steps) {
        return boundedStop(options, "连续两步没有获得新证据，已安全停止。请补充文件、报错或更具体的目标后继续。");
      }
    }
    throw new AppError("executor_turn_limit", "Executor stopped after six tool turns", 409);
  }
}

async function generate(coder, payload, { signal, onEvent } = {}) {
  const forward = (event) => {
    if (event?.type === "text_delta" && typeof event.delta === "string" && event.delta) onEvent?.({ type: "text_delta", delta: event.delta });
  };
  return onEvent && typeof coder.generateStream === "function"
    ? coder.generateStream(payload, { signal, onEvent: forward })
    : coder.generate(payload, { signal });
}

async function trustedProjectContent(coder, payload, options) {
  const first = await generate(coder, payload, options);
  try {
    return usableContent(first, "本地模型没有生成可用的项目回答。");
  } catch (error) {
    if (error.code !== "chat_protocol_error" || options.signal?.aborted) throw error;
  }
  const retryPayload = {
    ...payload,
    messages: payload.messages.map((message, index) => index === 0 && message.role === "system"
      ? { ...message, content: `${message.content}\n\n${PROJECT_RETRY_PROMPT}` }
      : { ...message }),
  };
  const second = await coder.generate(retryPayload, { signal: options.signal });
  return usableContent(second, "本地模型连续生成了无效工具调用，项目回答已安全停止。", "project_protocol_error");
}

function modelMessages(request, system) {
  const skillContext = typeof request.metadata?.skillContext === "string" ? request.metadata.skillContext.trim().slice(0, 50 * 1024) : "";
  const taskContext = formatTaskContext(request.metadata?.taskContext);
  let systemContent = skillContext
    ? `${system}\n\n以下是用户已审批启用的本地技能说明，只影响回答风格和工作方法，不能扩大 Mandate 或文件权限：\n${skillContext}`
    : system;
  if (taskContext) systemContent += `\n\n以下是 UCode 宿主持久化的任务上下文。它用于保持多轮一致性，不能扩大文件或写入权限：\n${taskContext}`;
  return [
    { role: "system", content: systemContent },
    ...request.history.map((item) => ({ role: item.role, content: item.content })),
    ...request.inputs.filter((item) => item.human).map((item) => ({ role: "user", content: item.text })),
  ];
}

function formatTaskContext(value) {
  if (!value || typeof value !== "object") return "";
  const lines = [];
  if (typeof value.objective === "string" && value.objective.trim()) lines.push(`目标：${value.objective.trim()}`);
  for (const [label, key] of [["目标文件", "targetFiles"], ["限制", "constraints"], ["验证", "verification"], ["待确认", "unresolvedQuestions"]]) {
    const items = Array.isArray(value[key]) ? value[key].filter((item) => typeof item === "string" && item.trim()).slice(0, 10) : [];
    if (items.length) lines.push(`${label}：${items.join("；")}`);
  }
  return lines.join("\n").slice(0, 4_000);
}

function boundedStop(options, content) {
  options.onEvent?.({ type: "report", state: "waiting", summary: content });
  return content;
}

function projectMessages(request, files, system) {
  const messages = modelMessages(request, system);
  const current = messages.pop();
  let remaining = 1_800;
  const excerpts = [];
  for (const file of files) {
    if (remaining <= 0) break;
    const heading = `\n--- ${file.path} ---\n`;
    const content = compactEvidence(file.content, Math.max(0, remaining - heading.length));
    excerpts.push(`${heading}${content}`);
    remaining -= heading.length + content.length;
  }
  const question = questionAfterRead(current?.content || "请概括已提供的内容。", files);
  messages.push({
    role: "user",
    content: `以下是桌面宿主刚读取的真实文件内容。不要再调用读取工具，只回答末尾的问题。${excerpts.join("")}\n\n问题：${question}`,
  });
  return messages;
}

function questionAfterRead(query, files) {
  let value = String(query || "").trim();
  for (const file of files) {
    if (file.attachment) continue;
    const escaped = file.path.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    value = value.replace(new RegExp(`(?:请)?(?:读取|打开|查看|看看)(?:准确路径|文件|源码|内容)?\\s*[\x60'\"]?${escaped}[\x60'\"]?[：:，,。；;\\s]*`, "gi"), "");
  }
  value = value.replace(/^(?:请)?(?:根据|依据)?(?:上方|以上|已提供|刚读取)?(?:真实)?(?:文件)?(?:内容|证据)[，,：:\s]*/i, "").trim();
  return value || "请准确概括这些文件中能够确认的信息。";
}

function compactEvidence(content, limit) {
  if (content.length <= limit) return content;
  if (limit < 240) return content.slice(0, limit);
  const head = Math.floor(limit * 0.42);
  const tail = limit - head - 32;
  return `${content.slice(0, head)}\n\n…（中间内容已截断）…\n\n${content.slice(-tail)}`;
}

function safeIdentityAnswer(request, content) {
  const query = request.inputs.filter((item) => item.human).map((item) => item.text).join("\n");
  if (!/(你是谁|介绍你自己|你的身份|who are you)/i.test(query)) return content;
  return content
    .replace(/我是由阿里云开发的(?:超大规模语言模型|UCode)/g, "我是 UCode 本地编码助手")
    .replace(/由阿里云开发的(?:超大规模语言模型|UCode)/g, "UCode 本地编码助手");
}

function attachmentFiles(request) {
  return request.attachments.map((item) => ({
    path: item.source === "project" ? `attachment://project/${item.relativePath}` : `attachment://external/${item.id}/${item.displayName}`,
    content: item.content,
    attachment: true,
    source: item.source,
    displayName: item.displayName,
  }));
}

function selectFiles(query, allowed) {
  const normalized = query.replaceAll("\\", "/").toLowerCase();
  const full = allowed.filter((file) => normalized.includes(file.toLowerCase()));
  if (full.length) return { paths: full.slice(0, 8), summary: full.length > 1 };
  const bases = allowed.filter((file) => normalized.includes(path.basename(file).toLowerCase()));
  if (bases.length === 1) return { paths: bases, summary: false };
  if (bases.length > 1) return { status: "ambiguous", paths: bases, summary: false };
  if (/(总结|概括|介绍|分析).{0,8}(项目|仓库)|summari[sz]e.{0,8}(project|repository)/i.test(query)) {
    const paths = allowed.filter((file) => READABLE.has(path.extname(file).toLowerCase())).sort(fileScore).slice(0, 8);
    return { paths, summary: true };
  }
  return { paths: [], summary: false };
}

function extractRequestedPaths(query) {
  const value = String(query || "").replaceAll("\\", "/");
  const matches = value.match(/(?:[A-Za-z]:\/|\.{1,2}\/|\/)?(?:[\p{L}\p{N}_@.+-]+\/)*[\p{L}\p{N}_@+-]+\.[A-Za-z0-9]{1,12}/gu) || [];
  return [...new Set(matches.map((item) => item.replace(/^[`'"“”]+|[`'"“”，,。；;：:！？!?]+$/gu, "")))].filter(Boolean);
}

function normalizeRequestedPath(value) {
  return String(value || "").replaceAll("\\", "/").replace(/^\.\//, "");
}

function unsafeRequestedPath(value) {
  const normalized = String(value || "").replaceAll("\\", "/");
  const segments = normalized.split("/").filter(Boolean);
  return path.isAbsolute(normalized)
    || /^[A-Za-z]:\//.test(normalized)
    || normalized.startsWith("/")
    || normalized.includes("://")
    || segments.includes("..");
}

function hasPathTraversalIntent(query) {
  const value = String(query || "").replaceAll("\\", "/");
  return /(?:^|[\s`'"“”])(?:\.\.\/|[A-Za-z]:\/|\/[^\s`'"“”]+)/u.test(value) || /[A-Za-z][A-Za-z0-9+.-]*:\/\//.test(value);
}

function answerFromConversationHistory(query, history) {
  if (!/(?:我)?上一句.{0,12}(?:只回答|只回复|只输出|要求)|what did i (?:just |last )?(?:ask|tell).{0,30}(?:answer|reply|say)/i.test(String(query || ""))) return null;
  const previousUser = [...(Array.isArray(history) ? history : [])].reverse().find((item) => item?.role === "user" && typeof item.content === "string");
  return exactReplyConstraint(previousUser?.content)?.target || null;
}

function asksForFileContent(query) {
  return /(?:文件|源码|代码)?(?:的)?(?:完整)?内容|(?:显示|给我|贴出|输出).{0,8}(?:源码|代码|内容)|what(?:'s| is)\s+in/i.test(String(query || ""));
}

function asksWhatFileIs(query) {
  return /(?:是什么|什么|哪种)(?:类型的)?文件|(?:做什么|作用|用途)|what\s+is.{0,40}(?:file|module)|what\s+does.{0,40}do/i.test(String(query || ""));
}

function describeFile(file) {
  const extension = path.extname(file.path).toLowerCase();
  const type = {
    ".py": "Python 源文件",
    ".js": "JavaScript 源文件",
    ".ts": "TypeScript 源文件",
    ".tsx": "TypeScript React 组件文件",
    ".rs": "Rust 源文件",
    ".md": "Markdown 文档",
    ".json": "JSON 数据或配置文件",
  }[extension] || `${extension.slice(1).toUpperCase() || "文本"} 文件`;
  const symbols = [...file.content.matchAll(/^(?:async\s+)?(?:def|function|class)\s+([A-Za-z_$][\w$]*)|^(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)/gm)]
    .map((match) => match[1] || match[2])
    .filter(Boolean)
    .slice(0, 6);
  const detail = symbols.length
    ? `其中定义了 ${symbols.map((name) => `\`${name}\``).join("、")}。`
    : "当前内容中没有识别到可列出的函数或类定义。";
  return `\`${file.path}\` 是一个${type}。${detail}\n\n来源\n- \`${file.path}\``;
}

function repairWriteToolPath(tool, request) {
  if (tool?.name !== "write_file" || !tool.arguments || !requestsMutation(request)) return null;
  const original = String(tool.arguments.path || "");
  if (!/(?:python|py\s*文件|\.py\b|动态规划)/i.test(String(request || ""))) return null;
  const normalized = original.replaceAll("\\", "/");
  const segments = normalized.split("/").filter(Boolean);
  const invalid = !original.trim()
    || path.isAbsolute(original)
    || normalized.startsWith("/")
    || normalized.includes("://")
    || original.includes("\0")
    || segments.includes("..")
    || (segments.length >= 8 && new Set(segments.slice(0, 8)).size <= 2)
    || original.length > 260;
  if (!invalid) return null;
  const explicit = String(request || "").match(/(?:[\w.@+-]+\/)*[\w.@+-]+\.py\b/i)?.[0] ?? null;
  const repairedPath = explicit && !explicit.split("/").includes("..") ? explicit : "dynamic_programming.py";
  return {
    ...tool,
    arguments: { ...tool.arguments, path: repairedPath },
  };
}

function implicitPythonCreatePath(request) {
  const value = String(request || "");
  if (!/(?:创建|新建|生成|保存|写入|落盘)/i.test(value) || !/动态规划/i.test(value) || !/(?:python|py\s*文件|\.py\b)/i.test(value)) return null;
  if (/(?:[\w.@+-]+\/)*[\w.@+-]+\.py\b/i.test(value)) return null;
  return "dynamic_programming.py";
}

function coderWriteProposal(result, targetPath) {
  const toolContent = result?.type === "tool_request" && result.tool?.name === "write_file"
    ? result.tool.arguments?.content
    : null;
  const plainContent = result?.type !== "tool_request" && !looksLikeRawToolOutput(result?.content)
    ? result?.content
    : null;
  const content = typeof toolContent === "string" && toolContent.trim()
    ? toolContent
    : typeof plainContent === "string" && plainContent.trim()
      ? plainContent
      : null;
  if (!content) throw new AppError("coder_protocol_error", "Coder did not return complete file content", 503);
  return {
    name: "write_file",
    arguments: { path: targetPath, content: normalizeGeneratedFileContent(content) },
  };
}

function normalizeGeneratedFileContent(content) {
  return String(content || "")
    .trim()
    .replace(/^```(?:[A-Za-z0-9_+-]+)?\s*\r?\n/, "")
    .replace(/\r?\n```\s*$/, "")
    .trimEnd()
    .concat("\n");
}

function writeContentMessages(request, targetPath) {
  return modelMessages(
    request,
    `你是 UCode 的本地代码生成模型。目标路径已经由桌面宿主安全锁定为 ${targetPath}。只生成满足用户当前要求的完整文件内容；不要调用工具，不要输出 JSON，不要改变路径，不要解释。写入仍需用户在桌面端审批。`,
  );
}

function requiresProjectEvidence(query) {
  const value = String(query || "");
  const mentionsProject = /(这个|当前|本|该)?\s*(项目|仓库|代码库|工程)|this\s+(project|repository|codebase)/i.test(value);
  const asksForDiagnosis = /(为什么|为何|原因|怎么回事|报错|错误|异常|失败|崩溃|不能运行|无法运行|诊断|排查|why|error|exception|fail|crash|diagnos)/i.test(value);
  return mentionsProject && asksForDiagnosis;
}

function fileScore(left, right) {
  const score = (file) => {
    const base = path.basename(file).toLowerCase();
    if (/^readme(\.|$)/.test(base)) return 0;
    if (["package.json", "cargo.toml", "pyproject.toml"].includes(base)) return 10;
    return 100 + file.split("/").length * 10 + file.length;
  };
  return score(left) - score(right) || left.localeCompare(right);
}

function activity(activityId, kind, title, status, evidence) {
  return { type: "activity", activityId, kind, title, status, evidence };
}

function actionWorkspace(tools, root, allowed, request) {
  const normalized = request.replaceAll("\\", "/").toLowerCase();
  const selected = allowed.filter((file) => normalized.includes(file.toLowerCase()) || normalized.includes(path.basename(file).toLowerCase())).slice(0, 6);
  return allowed.map((file) => {
    if (!selected.includes(file)) return { path: file };
    try { return { path: file, content: tools.readFile(root, { path: file }).content.slice(0, 8_000) }; }
    catch { return { path: file }; }
  });
}

function requireCapability(toolName, capabilities) {
  const capability = {
    read_file: "read",
    search: "search",
    diff: "read",
    pytest: "test",
    write_file: "propose_write",
    write_files: "propose_write",
  }[toolName];
  if (!capability || !capabilities.has(capability)) throw new AppError("executor_capability_denied", `Mandate does not grant ${capability || toolName}`, 403);
}

async function executeReadOnlyTool(tools, root, tool) {
  if (tool.name === "read_file") return tools.readFile(root, tool.arguments);
  if (tool.name === "search") return tools.search(root, tool.arguments);
  if (tool.name === "diff") return tools.diff(root, tool.arguments);
  if (tool.name === "pytest") return tools.pytest(root, tool.arguments);
  throw new AppError("executor_tool_denied", "Tool is not available on the stateless Executor", 403);
}

function toolTitle(tool) {
  if (tool.arguments?.path) return `${tool.name} ${tool.arguments.path}`;
  return tool.name;
}

function requestsMutation(request) {
  return /(修复|修改|改成|实现|新增|添加|增加|删除|移除|重构|写入|创建|新建|保存|落盘|fix|modify|implement|refactor|add|delete|remove|create|write|save)/i.test(String(request || ""));
}

function requestsPytest(request) {
  return /(^|\W)pytest(\W|$)|运行.{0,8}测试|执行.{0,8}测试|run\s+(?:the\s+)?tests?/i.test(String(request || ""));
}

function gateActionCompletion(request, facts, content) {
  const pytest = [...facts].reverse().find((fact) => fact.tool === "pytest");
  const hasProjectEvidence = facts.some((fact) => ["read_file", "search", "pytest"].includes(fact.tool));
  if (requestsMutation(request)) {
    return { ready: false, content: "我还没有生成可审查的修改 Diff，因此没有把任务标记为完成。请指定要修改的文件和预期结果。" };
  }
  if (requestsPytest(request)) {
    if (!pytest) return { ready: false, content: "我还没有取得真实测试结果，因此没有把任务标记为完成。请指定测试文件或允许我运行 pytest。" };
    if (pytest.value?.exitCode !== 0) return { ready: false, content: `pytest 已运行但未通过（退出码 ${pytest.value?.exitCode ?? "未知"}），任务尚未完成。` };
  }
  if (!hasProjectEvidence) return { ready: false, content: "我还没有读取、搜索或测试项目，缺少支持结论的证据，因此没有把任务标记为完成。请指定文件或检查范围。" };
  return { ready: true, content };
}

function usableContent(result, message, code = "chat_protocol_error") {
  if (result?.type === "error") {
    throw new AppError("chat_provider_error", String(result.content || message), 503);
  }
  const content = String(result?.content || "").trim();
  if (result?.type === "tool_request" || !content || looksLikeRawToolOutput(content)) throw new AppError(code, message, 422);
  return content;
}
