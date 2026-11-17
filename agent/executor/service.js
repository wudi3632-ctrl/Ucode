import path from "node:path";
import { createHash } from "node:crypto";
import { createTicket, initialMessages, assign, report, ReportKind, TicketState } from "../domain/domain.js";
import { assessTool, requireApproval } from "../policy/policy.js";
import { resolveProjectRoot } from "../local_source/workspace.js";
import { buildEscalationPayload } from "../escalation/escalation.js";
import { AppError } from "../shared/errors.js";
import { TOOL_DEFINITIONS } from "../tools/definitions.js";
import { Intent, classifyIntent, conversationMessages, looksLikeRawToolOutput, validateToolRequest } from "./router.js";
import { enforceOneSentence, exactReplyConstraint, oneSentenceConstraint } from "./chat_quality.js";

const CHAT_PROMPT = "你是 UCode 桌面应用中的本地编码助手，不是 Python 脚本，也不是用户项目中的代码。请直接、自然、简洁地使用中文回答。当前是普通聊天模式：不要输出工具调用、JSON 或虚构文件操作。保留并延续对话上下文。用户要求一句话时，只输出一个句号结尾的完整句子。";
const PROJECT_PROMPT = "你是 UCode 的项目问答助手。只能依据提供的项目文件索引和文件内容回答；信息不足时明确说明需要哪个文件，不得猜测路径，不得输出工具调用或 JSON，不得声称已经修改文件。展示代码时必须使用带语言名的 Markdown 围栏，禁止把换行写成字面量 \\n。来源路径由桌面宿主统一展示，不要在回答中重复来源。使用简洁中文。";
const PLAIN_TEXT_RETRY_PROMPT = "上一次输出没有遵守纯文本回答协议。本次只能输出面向用户的自然语言答案；禁止输出 JSON、XML、工具名称、函数调用或代码形式的工具参数。";
const TEXT_EXTENSIONS = new Set([".md", ".txt", ".js", ".mjs", ".cjs", ".ts", ".tsx", ".jsx", ".py", ".rs", ".go", ".java", ".kt", ".c", ".h", ".cpp", ".hpp", ".cs", ".json", ".jsonl", ".yaml", ".yml", ".toml", ".ini", ".cfg", ".xml", ".html", ".css", ".scss", ".sql", ".sh", ".ps1", ".bat", ".cmd", ".log", ".csv"]);

function toolMessageContent(name, result) {
  if (name === "read_file") return result.content;
  if (["search", "diff", "pytest"].includes(name)) return result.stdout || result.stderr || JSON.stringify(result);
  return JSON.stringify(result);
}

function formatTestFailureSummary(evidence, projectRoot) {
  const preview = sanitizeTestPreview(evidence?.preview, projectRoot);
  return preview ? `\n\n测试摘要：\n\`\`\`text\n${preview}\n\`\`\`` : "";
}

function sanitizeTestPreview(value, projectRoot) {
  let preview = typeof value === "string" ? value : "";
  if (!preview.trim()) return "";
  const rootVariants = [projectRoot, projectRoot?.replaceAll("\\", "/")].filter(Boolean);
  for (const root of rootVariants) preview = preview.replaceAll(root, ".");
  preview = preview
    .replace(/\u001b\[[0-?]*[ -\/]*[@-~]/g, "")
    .replace(/```/g, "'''")
    .replace(/\r\n?/g, "\n")
    .split("\n")
    .map((line) => line.replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, "").trimEnd())
    .filter((line, index, lines) => line.trim() || (index > 0 && index < lines.length - 1))
    .slice(0, 12)
    .join("\n")
    .trim()
    .slice(0, 420);
  return preview;
}

function evidencePreview(toolName, output) {
  const text = String(output || "").replace(/\r\n?/g, "\n");
  if (!text || toolName !== "pytest" || text.length <= 480) return text ? text.slice(0, 480) : null;
  const lines = text.split("\n").map((line) => line.trimEnd());
  const important = lines.filter((line) =>
    /FAILURES|ERRORS|FAILED|PASSED|AssertionError|assert\s|short test summary|^\s*E\s+|^\s*>\s+|^\s*test_[A-Za-z0-9_]+|_{3,}.*_{3,}/i.test(line),
  );
  const selected = [];
  for (const line of [...lines.slice(0, 3), ...important, ...lines.slice(-4)]) {
    if (line && !selected.includes(line)) selected.push(line);
  }
  return selected.join("\n").slice(0, 480);
}

export class AgentService {
  constructor({ store, audit, tools, model, coder = null, dataDir }) {
    this.store = store;
    this.audit = audit;
    this.tools = tools;
    this.model = model;
    this.coder = coder;
    this.externalCoder = null;
    this.dataDir = dataDir;
    this.pendingAttachmentContexts = new Map();
    this.activeAttachmentContexts = new Map();
  }

  setExternalCoder(client) { this.externalCoder = client; }
  activeCoder() { return this.externalCoder || this.coder; }

  selectProject(projectRoot) {
    this.store.state.projectRoot = resolveProjectRoot(projectRoot);
    this.store.save();
    this.audit.append("project.selected", null, { root: this.store.state.projectRoot });
    return { projectRoot: this.store.state.projectRoot };
  }

  createTicket(request) {
    if (!this.store.state.projectRoot) throw new AppError("project_required", "Select a project first", 409);
    if (typeof request !== "string" || !request.trim()) throw new AppError("invalid_request", "Task request is required");
    const ticket = createTicket(this.store.state.projectRoot, request.trim());
    this.store.state.tickets.push(ticket);
    this.store.save();
    this.audit.append("ticket.created", ticket.id, { request: ticket.trigger.request, mandate: ticket.mandate });
    return ticket;
  }

  continueTicket(id, request) {
    const ticket = this.ticket(id);
    if (typeof request !== "string" || !request.trim()) throw new AppError("invalid_request", "Message is required");
    ticket.execution ||= { messages: initialMessages(ticket.trigger.request), turns: 0, pendingEffectId: null, testsPassed: false };
    if (ticket.effects.some((effect) => effect.status === "recovery_required")) {
      throw new AppError("recovery_required", "This task is locked because a file transaction could not be restored and verified. Inspect retained backups before starting a new task.", 409);
    }
    if (ticket.execution.pendingEffectId) throw new AppError("approval_required", "Resolve the pending change before sending another message", 409);
    ticket.execution.messages.push({ role: "user", content: request.trim() });
    ticket.execution.turns = 0;
    ticket.state = TicketState.OPEN;
    ticket.updatedAt = new Date().toISOString();
    this.store.save();
    this.audit.append("message.created", id, { role: "user", content: request.trim() });
    return ticket;
  }

  hydrateTicket(id, messages) {
    const ticket = this.ticket(id);
    if (!Array.isArray(messages)) throw new AppError("invalid_request", "Conversation context must be an array");
    const normalized = messages.slice(-200).map((message) => {
      const role = message?.role;
      const content = typeof message?.content === "string" ? message.content.trim() : "";
      if (!["user", "assistant"].includes(role) || !content) {
        throw new AppError("invalid_request", "Conversation context accepts non-empty user and assistant messages only");
      }
      return { role, content: content.slice(0, 100_000) };
    });
    ticket.execution ||= { messages: initialMessages(ticket.trigger.request), turns: 0, pendingEffectId: null, testsPassed: false };
    ticket.execution.messages.push(...normalized);
    ticket.updatedAt = new Date().toISOString();
    this.store.save();
    this.audit.append("conversation.hydrated", id, { messageCount: normalized.length });
    return ticket;
  }

  setSkillContext(id, context) {
    const ticket = this.ticket(id);
    if (typeof context !== "string") throw new AppError("invalid_request", "Skill context must be a string");
    if (Buffer.byteLength(context, "utf8") > 50 * 1024) throw new AppError("skill_context_too_large", "Skill context exceeds 50 KiB", 413);
    ticket.execution ||= { messages: initialMessages(ticket.trigger.request), turns: 0, pendingEffectId: null, testsPassed: false };
    ticket.execution.skillContext = context.trim();
    ticket.updatedAt = new Date().toISOString();
    this.store.save();
    this.audit.append("skills.context_set", id, { bytes: Buffer.byteLength(ticket.execution.skillContext, "utf8") });
    return { bytes: Buffer.byteLength(ticket.execution.skillContext, "utf8") };
  }

  setAttachmentContext(id, attachments) {
    this.ticket(id);
    const normalized = normalizeAttachments(attachments);
    this.pendingAttachmentContexts.set(id, normalized);
    this.audit.append("attachments.context_set", id, {
      count: normalized.length,
      attachments: normalized.map(attachmentAuditMetadata),
    });
    return { count: normalized.length, bytes: normalized.reduce((total, item) => total + item.includedBytes, 0) };
  }

  listTickets() { return this.store.state.tickets; }

  ticket(id) {
    const ticket = this.store.ticket(id);
    if (!ticket) throw new AppError("ticket_not_found", "Ticket not found", 404);
    return ticket;
  }

  async plan(id, options = {}) {
    const ticket = this.ticket(id);
    const assignment = assign(ticket);
    this.audit.append("assignment.started", id, { assignmentId: assignment.id });
    ticket.execution ||= { messages: initialMessages(ticket.trigger.request), turns: 0, pendingEffectId: null, testsPassed: false };
    const activeAttachments = this.pendingAttachmentContexts.get(id) || [];
    this.pendingAttachmentContexts.delete(id);
    const activeAttachmentEvidence = activeAttachments.map((attachment) => {
      const evidence = attachmentEvidence(attachment);
      ticket.execution.evidence ||= [];
      ticket.execution.evidence.push(evidence);
      this.audit.append("attachment.consumed", id, attachmentAuditMetadata(attachment));
      options.onEvent?.({ type: "tool_completed", tool: "attachment_read", evidence });
      return evidence;
    });
    this.activeAttachmentContexts.set(id, { attachments: activeAttachments, evidence: activeAttachmentEvidence });
    const messages = ticket.execution.messages;
    try {
      const latestRequest = [...messages].reverse().find((message) => message.role === "user")?.content || ticket.trigger.request;
      let intent = classifyIntent(latestRequest, ticket.execution.intent);
      if (activeAttachments.length && intent !== Intent.TOOL_TASK) intent = Intent.PROJECT_QUESTION;
      ticket.execution.intent = intent;
      this.audit.append("route.selected", id, { intent });
      options.onEvent?.({ type: "route", intent });
      if (intent !== Intent.TOOL_TASK) return this.withAttachmentEvidence(ticket, await this.answerWithoutTools(ticket, latestRequest, intent, options));

      const taskRequest = `${ticket.trigger.request}\n${latestRequest}`;
      if (ticket.execution.directEditEffectId && !ticket.execution.pendingEffectId) {
        const resolved = ticket.effects.find((effect) => effect.id === ticket.execution.directEditEffectId);
        if (resolved?.status === "applied" && !requestsPytest(taskRequest)) {
          ticket.execution.directEditEffectId = null;
          const paths = resolved.changes?.map((change) => change.path) || [resolved.path];
          return this.finishAnswer(ticket, { type: "final", route: "explicit_edit", content: paths.length === 1
            ? `已应用对 \`${paths[0]}\` 的修改。`
            : `已应用 ${paths.length} 个文件的修改：${paths.map((item) => `\`${item}\``).join("、")}。` });
        }
        if (resolved?.status === "rejected") {
          ticket.execution.directEditEffectId = null;
          const paths = resolved.changes?.map((change) => change.path) || [resolved.path];
          return this.finishAnswer(ticket, { type: "final", route: "explicit_edit", content: paths.length === 1
            ? `已取消对 \`${paths[0]}\` 的修改，文件未写入。`
            : `已取消 ${paths.length} 个文件的修改，所有文件均未写入。` });
        }
      }

      const appliedDirectEffect = ticket.execution.directEditEffectId
        ? ticket.effects.find((effect) => effect.id === ticket.execution.directEditEffectId && effect.status === "applied")
        : null;
      if (!ticket.execution.testsPassed && requestsPytest(taskRequest) && appliedDirectEffect) {
        const tool = { name: "pytest", arguments: { paths: pytestPathsForEffect(appliedDirectEffect) } };
        messages.push({ role: "assistant", content: "", tool_calls: [tool] });
        const toolResult = await this.callTool(id, tool.name, tool.arguments);
        options.onEvent?.({ type: "tool_completed", tool: tool.name, evidence: toolResult.evidence });
        messages.push({ role: "tool", content: toolMessageContent(tool.name, toolResult.result) });
        if (toolResult.result.exitCode === 0) {
          ticket.execution.testsPassed = true;
          ticket.execution.directEditEffectId = null;
          ticket.execution.lastFailedTest = null;
          return this.withAttachmentEvidence(ticket, this.finishAnswer(ticket, {
            type: "final",
            route: "verified_edit",
            content: "修改已应用，并且 pytest 已通过。",
            evidence: [toolResult.evidence],
          }));
        }
        ticket.execution.directEditEffectId = null;
        ticket.execution.lastFailedTest = {
          effectId: appliedDirectEffect.id,
          exitCode: toolResult.result.exitCode ?? null,
          preview: toolResult.evidence.preview || null,
          sourcePaths: toolResult.evidence.sourcePaths || [],
          facts: toolResult.evidence.testFailure || null,
        };
        const failureSummary = formatTestFailureSummary(toolResult.evidence, ticket.projectRoot);
        return this.withAttachmentEvidence(ticket, this.finishAnswer(ticket, {
          type: "needs_clarification",
          route: "verified_edit",
          content: `修改已应用，但 pytest 未通过（退出码 ${toolResult.result.exitCode ?? "未知"}），任务尚未完成。${failureSummary}\n\n你可以继续修复，或回滚这批修改。`,
          evidence: [toolResult.evidence],
        }));
      }
      const failedTestRepair = await this.directFailedTestRepair(ticket, latestRequest, options);
      if (failedTestRepair) return this.withAttachmentEvidence(ticket, failedTestRepair);
      const generatedFile = await this.directGeneratedFile(ticket, latestRequest);
      if (generatedFile) return this.withAttachmentEvidence(ticket, generatedFile);
      const directEdit = await this.directExplicitEdit(ticket, latestRequest, options);
      if (directEdit) return this.withAttachmentEvidence(ticket, directEdit);
      while (ticket.execution.turns < 8) {
        const turn = ticket.execution.turns;
        const workspace = this.workspaceContext(ticket, latestRequest, { includeContent: false });
        let result;
        try {
          result = await this.model.generate({ task_id: id, messages: withSkillContext(messages, ticket.execution.skillContext), workspace, tools: TOOL_DEFINITIONS, mode: turn === 0 ? "plan" : "execute" }, { signal: options.signal });
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
        ticket.execution.turns += 1;
        this.audit.append("model.response", id, { turn, ...responseAuditMetadata(result) });
        if (result.type !== "tool_request") {
          if (looksLikeRawToolOutput(result.content)) {
            ticket.execution.protocolFailures = (ticket.execution.protocolFailures || 0) + 1;
            this.audit.append("model.protocol_rejected", id, { reason: "raw_tool_output", attempt: ticket.execution.protocolFailures });
            if (ticket.execution.protocolFailures <= 2) {
              messages.push({ role: "assistant", content: result.content || "" });
              messages.push({ role: "user", content: "上一个输出不是合法工具调用。只返回一个符合工具定义的调用；路径必须来自项目文件索引。" });
              continue;
            }
            throw new AppError("model_protocol_error", "本地模型连续生成了无效工具调用，已安全停止且未执行操作。", 422);
          }
          messages.push({ role: "assistant", content: result.content || "" });
          const normalized = result.type === "message" ? { ...result, type: "final" } : result;
          const gated = gateToolTaskCompletion(ticket, taskRequest, normalized);
          if (gated.type === "needs_clarification") return this.withAttachmentEvidence(ticket, this.finishAnswer(ticket, gated));
          report(ticket, gated.type === "error" ? ReportKind.FAILED : ReportKind.READY, gated);
          this.store.save();
          return this.withAttachmentEvidence(ticket, gated);
        }
        if (!result.tool || typeof result.tool.name !== "string" || typeof result.tool.arguments !== "object") {
          throw new AppError("model_protocol_error", "Model returned an invalid tool request", 503);
        }
        const availablePaths = workspace.files.filter((file) => !file.attachment).map((file) => file.path);
        let validatedTool = result.tool;
        try {
          validatedTool = validateToolRequest(result.tool, availablePaths);
        } catch (error) {
          const repaired = repairWriteToolPath(result.tool, latestRequest);
          if (repaired) {
            validatedTool = validateToolRequest(repaired, availablePaths);
            this.audit.append("tool.path_repaired", id, {
              rejectedPath: result.tool?.arguments?.path ?? null,
              repairedPath: repaired.arguments.path,
            });
          } else {
            ticket.execution.toolFailures = (ticket.execution.toolFailures || 0) + 1;
            this.audit.append("tool.rejected", id, { code: error.code || "invalid_tool", message: error.message, attempt: ticket.execution.toolFailures });
            if (ticket.execution.toolFailures <= 2) {
              messages.push({ role: "assistant", content: "", tool_calls: [result.tool] });
              messages.push({ role: "tool", content: JSON.stringify({ error: { code: error.code || "invalid_tool", message: error.message }, availableFiles: workspace.files.filter((file) => !file.attachment).slice(0, 80).map((file) => file.path) }) });
              continue;
            }
            throw new AppError("model_tool_failed", "本地模型连续生成了无效或越界路径，已安全停止且未执行操作。", 422);
          }
        }
        result.tool = validatedTool;
        if (result.tool.name === "write_file" && this.activeCoder()) {
          const controllerPath = result.tool.arguments.path;
          const coderResult = await this.activeCoder().generate({ task_id: id, messages: withSkillContext(messages, ticket.execution.skillContext), workspace: this.workspaceContext(ticket, latestRequest), tools: TOOL_DEFINITIONS, mode: "write_content", requested_tool: result.tool }, { signal: options.signal });
          this.audit.append("coder.response", id, responseAuditMetadata(coderResult));
          if (coderResult.type !== "tool_request" || coderResult.tool?.name !== "write_file" || typeof coderResult.tool.arguments?.content !== "string") {
            throw new AppError("coder_protocol_error", "Coder did not return a write_file request", 503);
          }
          result.tool = validateToolRequest({
            ...coderResult.tool,
            arguments: {
              ...coderResult.tool.arguments,
              path: controllerPath,
              content: normalizeCoderFileContent(coderResult.tool.arguments.content),
            },
          }, availablePaths);
        } else if (result.tool.name === "write_files" && this.activeCoder()) {
          const changes = result.tool.arguments.changes;
          if (!Array.isArray(changes)) throw new AppError("coder_protocol_error", "Controller did not provide a valid file batch", 503);
          for (const change of changes) {
            if (change?.operation === "delete" || typeof change?.content === "string") continue;
            const requestedTool = { name: "write_file", arguments: { path: change.path } };
            const changeWorkspace = this.workspaceContext(ticket, change.path);
            const coderResult = await this.activeCoder().generate({
              task_id: id,
              messages: withSkillContext(messages, ticket.execution.skillContext),
              workspace: changeWorkspace,
              tools: [],
              mode: "write_content",
              requested_tool: requestedTool,
            }, { signal: options.signal });
            this.audit.append("coder.response", id, { batchPath: change.path, ...responseAuditMetadata(coderResult) });
            if (coderResult.type !== "tool_request" || coderResult.tool?.name !== "write_file" || typeof coderResult.tool.arguments?.content !== "string") {
              throw new AppError("coder_protocol_error", `Coder did not return complete content for ${change.path}`, 503);
            }
            change.content = normalizeCoderFileContent(coderResult.tool.arguments.content);
          }
        }
        messages.push({ role: "assistant", content: "", tool_calls: [result.tool] });
        let toolResult;
        try {
          toolResult = await this.callTool(id, result.tool.name, result.tool.arguments);
        } catch (error) {
          ticket.execution.toolFailures = (ticket.execution.toolFailures || 0) + 1;
          this.audit.append("tool.failed", id, { name: result.tool.name, code: error.code || "tool_failed", message: error.message, attempt: ticket.execution.toolFailures });
          if (ticket.execution.toolFailures <= 2) {
            messages.push({ role: "tool", content: JSON.stringify({ error: { code: error.code || "tool_failed", message: error.message }, availableFiles: workspace.files.filter((file) => !file.attachment).slice(0, 80).map((file) => file.path) }) });
            continue;
          }
          throw new AppError("model_tool_failed", "本地模型连续选择了无效工具参数，已安全停止且未执行操作。", 422);
        }
        if (["write_file", "write_files"].includes(result.tool.name)) {
          ticket.execution.pendingEffectId = toolResult.effect.id;
          this.store.save();
          const count = toolResult.effect.changes?.length || 1;
          return this.withAttachmentEvidence(ticket, { type: "message", content: `${count} file change${count === 1 ? " is" : "s are"} waiting for user approval`, tool: result.tool, effect: toolResult.effect });
        }
        messages.push({ role: "tool", content: toolMessageContent(result.tool.name, toolResult.result) });
        options.onEvent?.({ type: "tool_completed", tool: result.tool.name, evidence: toolResult.evidence });
        if (result.tool.name === "pytest" && toolResult.result.exitCode === 0) ticket.execution.testsPassed = true;
      }
      throw new AppError("turn_limit", "Agent stopped after eight tool turns", 409);
    } catch (error) {
      const cancelled = options.signal?.aborted || error.code === "cancelled" || error.name === "AbortError";
      report(ticket, cancelled ? ReportKind.DROPPED : ReportKind.FAILED, { message: cancelled ? "Generation cancelled" : error.message });
      this.store.save();
      throw error;
    } finally {
      this.activeAttachmentContexts.delete(id);
      this.store.save();
    }
  }

  withAttachmentEvidence(ticket, result) {
    const items = this.activeAttachmentContexts.get(ticket.id)?.evidence || [];
    if (!items.length) return result;
    return { ...result, evidence: [...items, ...(Array.isArray(result.evidence) ? result.evidence : [])] };
  }

  async answerWithoutTools(ticket, request, intent, options = {}) {
    if (asksAboutCurrentAttachments(request)) {
      const attachments = this.activeAttachmentContexts.get(ticket.id)?.attachments || [];
      const content = attachments.length
        ? `本轮已附加 ${attachments.length} 个只读文件：\n\n${attachments.map((item) => `- ${item.displayName}（${item.source === "project" ? "项目" : "外部只读"}）`).join("\n")}`
        : "本轮没有附件。上轮附件不会自动复用，如需继续使用请重新添加。";
      return this.finishAnswer(ticket, { type: "final", route: intent, content, evidence: [] });
    }
    if (intent === Intent.LIST_FILES) return this.listProjectFiles(ticket, options);
    if (intent === Intent.READ_FILE) return this.readProjectFile(ticket, request, options);
    if (intent === Intent.SUMMARIZE_PROJECT) return this.summarizeProject(ticket, request, options);
    if (intent === Intent.PROJECT_QUESTION && /(能读取哪些|可以读取哪些|支持.{0,8}(文件|格式)|哪些文件.{0,8}(能|可以)读)/i.test(request)) {
      return this.finishAnswer(ticket, {
        type: "final",
        route: intent,
        content: "我可以直接读取 Markdown、TXT、常见代码文件、JSON、YAML、TOML、CSV 和日志等文本格式。PDF、DOCX 与图片需要专用解析器；当前 Agent 不会把这些二进制文件伪装成已读取，遇到时会明确说明能力限制。",
        evidence: [],
      });
    }
    const projectContext = intent === Intent.PROJECT_QUESTION
      ? this.projectQuestionContext(ticket, request, options)
      : null;
    if (projectContext?.clarification) return this.finishAnswer(ticket, projectContext.clarification);
    const workspace = projectContext?.workspace || { root: ticket.projectRoot, files: [] };
    if (!this.activeCoder()) throw new AppError("chat_provider_unavailable", "对话模型尚未启动。", 503);
    const prompt = skillPrompt(intent === Intent.PROJECT_QUESTION ? PROJECT_PROMPT : CHAT_PROMPT, ticket.execution.skillContext);
    const messages = conversationMessages(ticket.execution.messages, prompt);
    const content = intent === Intent.PROJECT_QUESTION
      ? await this.generateTrustedProjectText(ticket, { messages, workspace, mode: intent, options, failureMessage: "本地对话模型没有生成可用的项目回答，请重试或切换模型。" })
      : await this.generateChatText(ticket, { messages, workspace, mode: intent, options });
    const grounded = intent === Intent.PROJECT_QUESTION ? groundProjectAnswer(content, workspace) : content;
    return this.finishAnswer(ticket, { type: "final", content: grounded, route: intent, evidence: projectContext?.evidence || [] });
  }

  projectQuestionContext(ticket, request, options = {}) {
    const index = this.tools.listFiles(ticket.projectRoot);
    const requested = [...new Set(extractRequestedPaths(request))];
    const selectedPaths = [];
    for (const requestedPath of requested) {
      const lower = requestedPath.toLowerCase();
      const exact = index.find((item) => item.toLowerCase() === lower);
      const basenameMatches = index.filter((item) => path.basename(item).toLowerCase() === lower);
      if (!exact && basenameMatches.length > 1) {
        return { clarification: {
          type: "needs_clarification",
          route: Intent.PROJECT_QUESTION,
          content: `找到多个名为 \`${requestedPath}\` 的文件，请指定其中一个：\n\n${basenameMatches.map((item) => `- ${item}`).join("\n")}`,
          evidence: [],
        } };
      }
      const resolved = exact || basenameMatches[0];
      if (!resolved) {
        return { clarification: {
          type: "needs_clarification",
          route: Intent.PROJECT_QUESTION,
          content: `项目索引中不存在 \`${requestedPath}\`。请提供存在的项目相对路径。`,
          evidence: [],
        } };
      }
      if (isReadableText(resolved) && !selectedPaths.includes(resolved)) selectedPaths.push(resolved);
    }
    if (!selectedPaths.length && isProjectQuestionFollowUp(request)) {
      selectedPaths.push(...(ticket.execution.lastResolvedPaths || []).filter((item) => index.includes(item) && isReadableText(item)).slice(0, 4));
    }
    const attachments = (this.activeAttachmentContexts.get(ticket.id)?.attachments || []).map((attachment) => ({
      path: attachmentVirtualPath(attachment),
      content: attachment.content,
      attachment: true,
      displayName: attachment.displayName,
      source: attachment.source,
      sha256: attachment.sha256,
      truncated: attachment.truncated,
    }));
    if (!selectedPaths.length && !attachments.length) {
      return { clarification: {
        type: "needs_clarification",
        route: Intent.PROJECT_QUESTION,
        content: "我还没有读取到能支持这个回答的文件。请指定文件名，或让我“总结本项目”，我会先读取代表性文件再回答。",
        evidence: [],
      } };
    }
    const selected = [];
    const evidence = [];
    let remaining = 4_800;
    for (const pathName of selectedPaths.slice(0, 4)) {
      const startedAt = new Date().toISOString();
      const result = this.tools.readFile(ticket.projectRoot, { path: pathName });
      const content = result.content.slice(0, Math.max(0, remaining));
      remaining -= content.length;
      selected.push({ path: pathName, content });
      const item = this.recordEvidence(ticket, "read_file", { path: pathName }, result, startedAt);
      evidence.push(item);
      options.onEvent?.({ type: "tool_completed", tool: "read_file", evidence: item });
      if (remaining <= 0) break;
    }
    if (selected.length) ticket.execution.lastResolvedPaths = selected.map((item) => item.path);
    const selectedMap = new Map(selected.map((item) => [item.path, item.content]));
    const files = index.map((item) => selectedMap.has(item) ? { path: item, content: selectedMap.get(item) } : { path: item });
    return { workspace: { root: ticket.projectRoot, files: [...files, ...attachments] }, evidence };
  }

  async generateChatText(ticket, { messages, workspace, mode, options }) {
    const currentRequest = [...messages].reverse().find((message) => message.role === "user")?.content || "";
    const exact = exactReplyConstraint(currentRequest);
    if (exact) return this.generateExactReply(ticket, { exact, workspace, mode, options });
    const singleSentence = oneSentenceConstraint(currentRequest);
    const result = options.onEvent && !singleSentence
      ? await this.activeCoder().generateStream({ task_id: ticket.id, messages, workspace, tools: [], mode }, { signal: options.signal, onEvent: options.onEvent })
      : await this.activeCoder().generate({ task_id: ticket.id, messages, workspace, tools: [], mode }, { signal: options.signal });
    this.audit.append("coder.response", ticket.id, { intent: mode, ...responseAuditMetadata(result) });
    const content = singleSentence ? enforceOneSentence(result.content) : String(result.content || "").trim();
    if (!content || looksLikeRawToolOutput(content)) throw new AppError("chat_protocol_error", "本地对话模型没有生成可用的文本回答，请重试或切换模型。", 422);
    if (singleSentence) options.onEvent?.({ type: "text_delta", delta: content });
    return content;
  }

  async generateExactReply(ticket, { exact, workspace, mode, options }) {
    if (options.signal?.aborted) throw new AppError("cancelled", "Generation cancelled", 499);
    const content = exact.target;
    options.onEvent?.({ type: "text_delta", delta: content });
    this.audit.append("host.exact_reply", ticket.id, { intent: mode, deterministic: true, contentLength: content.length });
    return content;
  }

  async generateTrustedProjectText(ticket, { messages, workspace, mode, options, failureMessage }) {
    for (let attempt = 1; attempt <= 2; attempt += 1) {
      if (options.signal?.aborted) throw new AppError("cancelled", "Generation cancelled", 499);
      const retryMessages = attempt === 1 ? messages : forcePlainText(messages);
      const result = attempt === 1 && options.onEvent
        ? await this.activeCoder().generateStream({ task_id: ticket.id, messages: retryMessages, workspace, tools: [], mode }, { signal: options.signal, onEvent: options.onEvent })
        : await this.activeCoder().generate({ task_id: ticket.id, messages: retryMessages, workspace, tools: [], mode }, { signal: options.signal });
      this.audit.append("coder.response", ticket.id, { intent: mode, attempt, ...responseAuditMetadata(result) });
      if (result?.type === "cancelled" || options.signal?.aborted) throw new AppError("cancelled", "Generation cancelled", 499);
      const content = String(result?.content || "").trim();
      if (result?.type !== "tool_request" && content && !looksLikeRawToolOutput(content)) return content;
      this.audit.append("model.protocol_rejected", ticket.id, { reason: "project_text_required", attempt });
    }
    throw new AppError("chat_protocol_error", failureMessage, 422);
  }

  listProjectFiles(ticket, options = {}) {
    const startedAt = new Date().toISOString();
    const paths = this.tools.listFiles(ticket.projectRoot);
    const evidence = this.recordEvidence(ticket, "list_files", {}, {
      paths,
      count: paths.length,
    }, startedAt);
    options.onEvent?.({ type: "tool_completed", tool: "list_files", evidence });
    const shown = paths.slice(0, 120);
    const remainder = paths.length - shown.length;
    const content = paths.length
      ? `当前项目索引包含 ${paths.length} 个文件：\n\n${shown.map((file) => `- ${file}`).join("\n")}${remainder > 0 ? `\n\n另有 ${remainder} 个文件未在消息中展开。` : ""}`
      : "当前项目中没有可读取的文件。";
    return this.finishAnswer(ticket, { type: "final", route: Intent.LIST_FILES, content, evidence: [evidence] });
  }

  async readProjectFile(ticket, request, options = {}) {
    const index = this.tools.listFiles(ticket.projectRoot);
    const resolution = resolveRequestedFile(request, index, ticket.execution.lastResolvedPaths || []);
    if (resolution.status === "ambiguous") {
      return this.finishAnswer(ticket, {
        type: "needs_clarification",
        route: Intent.READ_FILE,
        content: `找到多个同名文件，请指定其中一个：\n\n${resolution.candidates.map((file) => `- ${file}`).join("\n")}`,
        evidence: [],
      });
    }
    if (resolution.status === "missing") {
      const suggestions = index.filter(isReadableText).slice(0, 12);
      const lead = resolution.requested
        ? `项目索引中不存在 \`${resolution.requested}\`。`
        : "没有找到唯一的目标文件。";
      return this.finishAnswer(ticket, {
        type: "needs_clarification",
        route: Intent.READ_FILE,
        content: `${lead}请提供存在的相对路径，例如：\n\n${suggestions.map((file) => `- ${file}`).join("\n") || "项目中没有可读取的文本文件。"}`,
        evidence: [],
      });
    }
    const pathName = resolution.path;
    const startedAt = new Date().toISOString();
    const result = this.tools.readFile(ticket.projectRoot, { path: pathName });
    const evidence = this.recordEvidence(ticket, "read_file", { path: pathName }, result, startedAt);
    ticket.execution.lastResolvedPaths = [pathName];
    options.onEvent?.({ type: "tool_completed", tool: "read_file", evidence });

    if (asksForFileContent(request)) {
      const preview = result.content.length > 12_000 ? `${result.content.slice(0, 12_000)}\n\n[内容已截断，可在文件面板中继续查看]` : result.content;
      const language = markdownLanguage(pathName);
      const content = `\`${pathName}\` 的内容如下：\n\n\`\`\`${language}\n${preview.trimEnd()}\n\`\`\``;
      return this.finishAnswer(ticket, { type: "final", route: Intent.READ_FILE, content, evidence: [evidence] });
    }

    if (asksWhatFileIs(request)) {
      return this.finishAnswer(ticket, {
        type: "final",
        route: Intent.READ_FILE,
        content: describeProjectFile(pathName, result.content),
        evidence: [evidence],
      });
    }

    if (isReadFollowUp(request)) {
      if (!this.activeCoder()) throw new AppError("chat_provider_unavailable", "对话模型尚未启动。", 503);
      const workspace = { root: ticket.projectRoot, files: [{ path: pathName, content: result.content.slice(0, 1_200) }] };
      const messages = conversationMessages(ticket.execution.messages, skillPrompt(`${PROJECT_PROMPT}\n当前问题只针对已读取的 ${pathName}，回答必须引用这个来源。`, ticket.execution.skillContext));
      const content = await this.generateTrustedProjectText(ticket, { messages, workspace, mode: "project_question", options, failureMessage: "本地模型没有生成可用的文件解释。" });
      return this.finishAnswer(ticket, { type: "final", route: Intent.READ_FILE, content: withSources(content, [pathName]), evidence: [evidence] });
    }

    const preview = result.content.length > 12_000 ? `${result.content.slice(0, 12_000)}\n\n[内容已截断，可在文件面板中继续查看]` : result.content;
    const language = markdownLanguage(pathName);
    const content = `已读取 \`${pathName}\`。\n\n\`\`\`${language}\n${preview.trimEnd()}\n\`\`\``;
    return this.finishAnswer(ticket, { type: "final", route: Intent.READ_FILE, content, evidence: [evidence] });
  }

  async summarizeProject(ticket, request, options = {}) {
    if (!this.activeCoder()) throw new AppError("chat_provider_unavailable", "对话模型尚未启动。", 503);
    const index = this.tools.listFiles(ticket.projectRoot);
    const selected = selectRepresentativeFiles(index);
    const files = [];
    const evidence = [];
    let totalCharacters = 0;
    for (const pathName of selected) {
      if (totalCharacters >= 1_600) break;
      try {
        const startedAt = new Date().toISOString();
        const result = this.tools.readFile(ticket.projectRoot, { path: pathName });
        const remaining = Math.max(0, 1_600 - totalCharacters);
        const perFile = Math.min(remaining, 400);
        const content = result.content.slice(0, perFile);
        totalCharacters += content.length;
        files.push({ path: pathName, content });
        const item = this.recordEvidence(ticket, "read_file", { path: pathName }, result, startedAt);
        evidence.push(item);
        options.onEvent?.({ type: "tool_completed", tool: "read_file", evidence: item });
      } catch (error) {
        this.audit.append("tool.failed", ticket.id, { name: "read_file", path: pathName, message: error.message });
      }
    }
    if (!files.length) throw new AppError("project_evidence_unavailable", "没有读取到可用于总结的项目文件，因此没有生成项目结论。", 422);
    ticket.execution.lastResolvedPaths = files.map((file) => file.path);
    const messages = conversationMessages(ticket.execution.messages, skillPrompt(`${PROJECT_PROMPT}\n先说明项目用途和结构，再列出关键文件。所有结论必须来自已提供内容，结尾列出来源路径。`, ticket.execution.skillContext));
    const workspace = { root: ticket.projectRoot, files };
    const content = await this.generateTrustedProjectText(ticket, { messages, workspace, mode: "summarize_project", options, failureMessage: "本地模型没有生成可用的项目总结。" });
    return this.finishAnswer(ticket, { type: "final", route: Intent.SUMMARIZE_PROJECT, content: withSources(content, files.map((file) => file.path)), evidence });
  }

  finishAnswer(ticket, answer) {
    ticket.execution.messages.push({ role: "assistant", content: answer.content });
    report(ticket, answer.type === "needs_clarification" ? ReportKind.WAITING : ReportKind.READY, answer);
    this.store.save();
    return { tool: null, ...answer };
  }

  recordEvidence(ticket, toolName, argumentsValue, result, startedAt) {
    const output = typeof result.content === "string"
      ? result.content
      : [result.stdout, result.stderr].filter((item) => typeof item === "string" && item).join("\n");
    const sourcePaths = argumentsValue.path
      ? [argumentsValue.path]
      : Array.isArray(argumentsValue.paths)
        ? argumentsValue.paths
        : toolName === "search"
          ? searchResultPaths(result.stdout)
          : (result.paths || []);
    const evidence = {
      toolName,
      arguments: argumentsValue,
      startedAt,
      finishedAt: new Date().toISOString(),
      status: "succeeded",
      sourcePaths,
      contentDigest: result.sha256 || (output ? createHash("sha256").update(output).digest("hex") : null),
      preview: evidencePreview(toolName, output),
      exitCode: Number.isInteger(result.exitCode) ? result.exitCode : null,
      error: null,
    };
    if (toolName === "pytest") {
      evidence.testFailure = extractPytestFailureFacts(output, ticket.projectRoot, this.tools.listFiles(ticket.projectRoot));
    }
    ticket.execution.evidence ||= [];
    ticket.execution.evidence.push(evidence);
    this.audit.append("tool.completed", ticket.id, { name: toolName, arguments: argumentsValue, evidence });
    this.store.save();
    return evidence;
  }

  async directFailedTestRepair(ticket, request, options = {}) {
    const failure = ticket.execution.lastFailedTest;
    if (!failure || !isTestRepairFollowUp(request) || ticket.execution.pendingEffectId) return null;
    const effect = ticket.effects.find((item) => item.id === failure.effectId && item.status === "applied");
    const paths = (effect?.changes || [])
      .filter((change) => change.operation !== "delete" && typeof change.path === "string")
      .map((change) => change.path);
    if (paths.length !== 1) return null;
    const targetPath = paths[0];
    const startedAt = new Date().toISOString();
    const readResult = this.tools.readFile(ticket.projectRoot, { path: targetPath });
    const readEvidence = this.recordEvidence(ticket, "read_file", { path: targetPath }, readResult, startedAt);
    options.onEvent?.({ type: "tool_completed", tool: "read_file", evidence: readEvidence });
    const failureSummary = sanitizeTestPreview(failure.preview, ticket.projectRoot) || `pytest 退出码 ${failure.exitCode ?? "未知"}`;
    const failureFacts = failure.facts || extractPytestFailureFacts(failure.preview, ticket.projectRoot, this.tools.listFiles(ticket.projectRoot));
    const testContexts = [];
    for (const testPath of (failureFacts.testPaths || []).slice(0, 2)) {
      const testStartedAt = new Date().toISOString();
      const testResult = this.tools.readFile(ticket.projectRoot, { path: testPath });
      const testEvidence = this.recordEvidence(ticket, "read_file", { path: testPath }, testResult, testStartedAt);
      options.onEvent?.({ type: "tool_completed", tool: "read_file", evidence: testEvidence });
      testContexts.push({ path: testPath, content: testResult.content, readOnly: true });
    }
    const language = path.extname(targetPath).toLowerCase() || "原文件";
    const systemPrompt = skillPrompt(`你是 UCode 的测试驱动修复模型。唯一允许修改的路径是 ${targetPath}，目标文件类型是 ${language}。其他工作区文件只是已证明的只读测试上下文，不得修改。依据提供的真实当前文件、结构化失败事实和 pytest 摘要修复实现，同时保留此前已经批准的修改、原语言、既有导出以及全部无关接口。不得修改测试来规避失败。只输出修改后的完整 ${targetPath} 文件内容，不要解释，不要输出 JSON 或 Markdown 代码围栏。写入仍需用户批准。`, ticket.execution.skillContext);
    const userPrompt = `本轮要求：${request}\n\n结构化 pytest 失败事实：\n${formatPytestFailureFacts(failureFacts)}\n\npytest 失败摘要：\n${failureSummary}`;
    const workspace = { root: ticket.projectRoot, files: [{ path: targetPath, content: readResult.content }, ...testContexts] };
    const requestedTool = { name: "write_file", arguments: { path: targetPath } };
    let coderResult = null;
    let lastCandidateError = null;
    for (let attempt = 1; attempt <= 2; attempt += 1) {
      const retryPrompt = ["candidate_no_change", "candidate_no_effect"].includes(lastCandidateError?.code)
        ? `上一个候选没有对 ${targetPath} 的可执行实现产生有效变化，也没有修复任何失败。必须根据失败断言修改实现；只返回包含全部既有代码且确实改变实现的完整 ${language} 文件。`
        : `上一个候选不是 ${targetPath} 的完整有效文件。重新依据失败断言修复实现，只返回包含全部既有代码的完整 ${language} 文件。`;
      const messages = [
        { role: "system", content: systemPrompt },
        { role: "user", content: userPrompt },
        ...(attempt === 2 ? [{ role: "user", content: retryPrompt }] : []),
      ];
      const generated = await this.activeCoder().generate({ task_id: ticket.id, messages, workspace, tools: [], mode: "write_content", requested_tool: requestedTool });
      this.audit.append("coder.response", ticket.id, { route: "failed_test_repair", attempt, ...responseAuditMetadata(generated) });
      if (generated.type !== "tool_request" || generated.tool?.name !== "write_file" || typeof generated.tool.arguments?.content !== "string") continue;
      generated.tool.arguments.path = targetPath;
      generated.tool.arguments.content = normalizeCoderFileContent(generated.tool.arguments.content);
      try {
        this.tools.validateReplacement(targetPath, readResult.content, generated.tool.arguments.content);
        validateRepairCodeChange(targetPath, readResult.content, generated.tool.arguments.content);
        coderResult = generated;
        break;
      } catch (error) {
        lastCandidateError = error;
        this.audit.append("coder.candidate_rejected", ticket.id, { route: "failed_test_repair", path: targetPath, attempt, code: error.code || "candidate_invalid", message: error.message });
      }
    }
    if (!coderResult) {
      const recovery = ["candidate_no_change", "candidate_no_effect"].includes(lastCandidateError?.code)
        ? {
            reason: lastCandidateError.code,
            path: targetPath,
            attempts: 2,
            actions: ["attach_context", "configure_model"],
          }
        : null;
      return this.finishAnswer(ticket, {
        type: "needs_clarification",
        route: "failed_test_repair",
        content: ["candidate_no_change", "candidate_no_effect"].includes(lastCandidateError?.code)
          ? `本地代码模型连续两次都没有为 \`${targetPath}\` 生成有效的实现变化。UCode 已安全停止：没有创建待审批修改，也没有重复运行 pytest。当前轻量模型可能无法完成这次跨测试推理；你可以补充相关实现文件、更换更强模型，或回滚上一批修改。`
          : `本地代码模型没有为 \`${targetPath}\` 生成可审查的完整修复文件，因此没有写入，也没有重复运行 pytest。请补充期望的实现方式，或回滚上一批修改。`,
        evidence: [readEvidence],
        ...(recovery ? { recovery } : {}),
      });
    }
    const toolResult = await this.callTool(ticket.id, "write_file", coderResult.tool.arguments);
    ticket.execution.messages.push(
      { role: "assistant", content: "", tool_calls: [{ name: "read_file", arguments: { path: targetPath } }] },
      { role: "tool", content: readResult.content },
      { role: "assistant", content: "", tool_calls: [coderResult.tool] },
    );
    ticket.execution.pendingEffectId = toolResult.effect.id;
    ticket.execution.directEditEffectId = toolResult.effect.id;
    this.store.save();
    const result = { type: "message", tool: coderResult.tool, effect: toolResult.effect };
    this.audit.append("test.repair_proposed", ticket.id, {
      priorEffectId: failure.effectId,
      nextEffectId: toolResult.effect.id,
      path: targetPath,
      exitCode: failure.exitCode,
    });
    return {
      ...result,
      route: "failed_test_repair",
      content: `已根据上一轮 pytest 失败上下文重新读取 \`${targetPath}\`，并生成下一轮修复 Diff。批准后才会写入并重新运行测试。`,
    };
  }

  async directExplicitEdit(ticket, request, options = {}) {
    if (!this.activeCoder() || ticket.effects.some((effect) => effect.status === "pending")) return null;
    if (!/(修复|修改|改成|重构|新增|添加|增加|删除|移除|新建|创建|生成|fix|modify|refactor|add|delete|remove|create)/i.test(request)) return null;
    const explicitTargets = resolveExplicitBatchTargets(this.tools.listFiles(ticket.projectRoot), request);
    if (explicitTargets?.length > 1) {
      const targets = explicitTargets.map((target) => {
        if (target.operation === "create") return target;
        const startedAt = new Date().toISOString();
        const readResult = this.tools.readFile(ticket.projectRoot, { path: target.path });
        const evidence = this.recordEvidence(ticket, "read_file", { path: target.path }, readResult, startedAt);
        options.onEvent?.({ type: "tool_completed", tool: "read_file", evidence });
        return { ...target, content: readResult.content };
      });
      return this.directExplicitBatchEdit(ticket, request, targets);
    }
    const workspace = this.workspaceContext(ticket, request);
    const selected = workspace.files.filter((file) => typeof file.content === "string" && !file.attachment);
    if (!selected.length || selected.length > 20) return null;
    const requestedPaths = extractRequestedPaths(request);
    if (selected.length > 1 && requestedPaths.some((requested) => !selected.some((file) => file.path.toLowerCase() === requested.toLowerCase() || path.basename(file.path).toLowerCase() === requested.toLowerCase()))) return null;
    if (selected.length > 1) return this.directExplicitBatchEdit(ticket, request, selected);
    const target = selected[0];
    const deterministic = deterministicReplacement(target.path, target.content, request);
    if (deterministic) {
      this.tools.validateReplacement(target.path, target.content, deterministic);
      const tool = { name: "write_file", arguments: { path: target.path, content: deterministic } };
      const toolResult = await this.callTool(ticket.id, tool.name, tool.arguments);
      ticket.execution.messages.push(
        { role: "assistant", content: "", tool_calls: [{ name: "read_file", arguments: { path: target.path } }] },
        { role: "tool", content: target.content },
        { role: "assistant", content: "", tool_calls: [tool] },
      );
      ticket.execution.pendingEffectId = toolResult.effect.id;
      ticket.execution.directEditEffectId = toolResult.effect.id;
      this.audit.append("coder.deterministic_edit", ticket.id, { strategy: "insert_top_comment", path: target.path });
      this.store.save();
      return { type: "message", content: `已读取 ${target.path} 并生成修改，正在等待你的批准。`, tool, effect: toolResult.effect, route: "explicit_edit" };
    }
    const language = path.extname(target.path).toLowerCase() || "原文件";
    const messages = conversationMessages(ticket.execution.messages, skillPrompt(`你是 UCode 的代码修改模型。依据用户要求和已提供的真实文件内容，只输出修改后的完整文件内容。目标文件类型是 ${language}，必须保持原语言、既有导出和全部无关接口。不要解释，不要输出 JSON 或 Markdown 代码围栏。写入仍需用户批准。`, ticket.execution.skillContext));
    const requestedTool = { name: "write_file", arguments: { path: target.path } };
    let coderResult = null;
    for (let attempt = 1; attempt <= 2; attempt += 1) {
      const attemptMessages = attempt === 1 ? messages : [...messages, { role: "user", content: `上一个候选文件未通过质量检查。${target.path} 是 ${language} 文件；必须保持原语言、全部既有导出、函数和类，只修复用户指出的问题，并返回完整文件内容。` }];
      coderResult = await this.activeCoder().generate({ task_id: ticket.id, messages: attemptMessages, workspace, tools: [], mode: "write_content", requested_tool: requestedTool });
      this.audit.append("coder.response", ticket.id, { route: "explicit_edit", attempt, ...responseAuditMetadata(coderResult) });
      if (coderResult.type !== "tool_request" || coderResult.tool?.name !== "write_file" || typeof coderResult.tool.arguments?.content !== "string") continue;
      coderResult.tool.arguments.path = target.path;
      coderResult.tool.arguments.content = normalizeCoderFileContent(coderResult.tool.arguments.content);
      try {
        this.tools.validateReplacement(target.path, target.content, coderResult.tool.arguments.content);
        break;
      } catch (error) {
        const completed = completeAdditiveCandidate(target.path, target.content, coderResult.tool.arguments.content, request);
        if (completed) {
          try {
            this.tools.validateReplacement(target.path, target.content, completed);
            coderResult.tool.arguments.content = completed;
            this.audit.append("coder.candidate_completed", ticket.id, { attempt, strategy: "append_new_python_definition" });
            break;
          } catch {
            // Preserve the original validation failure below.
          }
        }
        this.audit.append("coder.candidate_rejected", ticket.id, { attempt, code: error.code || "candidate_invalid", message: error.message });
        coderResult = null;
      }
    }
    if (!coderResult) throw new AppError("coder_candidate_failed", "本地代码模型连续生成了不完整或无效的文件内容，已安全停止。", 422);
    validateToolRequest(coderResult.tool, workspace.files.filter((file) => !file.attachment).map((file) => file.path));
    ticket.execution.messages.push(
      { role: "assistant", content: "", tool_calls: [{ name: "read_file", arguments: { path: target.path } }] },
      { role: "tool", content: target.content },
      { role: "assistant", content: "", tool_calls: [coderResult.tool] },
    );
    const toolResult = await this.callTool(ticket.id, "write_file", coderResult.tool.arguments);
    ticket.execution.pendingEffectId = toolResult.effect.id;
    ticket.execution.directEditEffectId = toolResult.effect.id;
    this.store.save();
    return { type: "message", content: `已读取 ${target.path} 并生成修改，正在等待你的批准。`, tool: coderResult.tool, effect: toolResult.effect, route: "explicit_edit" };
  }

  async directExplicitBatchEdit(ticket, request, selected) {
    const changes = [];
    for (const target of selected) {
      if (target.operation === "delete") {
        changes.push({ path: target.path, operation: "delete" });
        continue;
      }
      const language = path.extname(target.path).toLowerCase() || "原文件";
      const action = target.operation === "create" ? "创建" : "修改";
      const scopedRequest = scopedRequestForTarget(request, target);
      const relatedImplementations = relatedImplementationPaths(request, target.path);
      const testRequirement = relatedImplementations.length
        ? `这是 pytest 测试文件：必须从 ${relatedImplementations.join("、")} 导入并测试真实实现；至少包含一个 def test_*() 函数和 assert；必须覆盖用户点名的每个函数调用；禁止在测试文件中复制被测函数实现。`
        : "";
      const deterministic = target.operation === "modify"
        ? deterministicReplacement(target.path, target.content, scopedRequest)
        : null;
      if (deterministic) {
        this.tools.validateReplacement(target.path, target.content, deterministic);
        validateExplicitCandidateRequirements(target.path, deterministic, scopedRequest);
        changes.push({ path: target.path, operation: target.operation, content: deterministic });
        this.audit.append("coder.deterministic_batch_file", ticket.id, {
          path: target.path,
          strategy: "literal_or_append",
        });
        continue;
      }
      const systemPrompt = skillPrompt(`你是 UCode 的代码修改模型。当前只${action} ${target.path}。依据下面这一条文件专属要求${target.operation === "create" ? "生成新文件" : "和已提供的真实文件内容完成修改"}，只输出该文件的完整内容。目标文件类型是 ${language}${target.operation === "modify" ? "，必须保持原语言、既有导出和全部无关接口" : ""}。${testRequirement}不要解释，不要输出 JSON 或 Markdown 代码围栏。其他文件会单独处理，写入仍需用户批准。`, ticket.execution.skillContext);
      const messages = [{ role: "system", content: systemPrompt }, { role: "user", content: `${scopedRequest}${testRequirement ? `\n${testRequirement}` : ""}` }];
      const requestedTool = { name: "write_file", arguments: { path: target.path } };
      const dependencyCandidates = changes
        .filter((change) => change.operation !== "delete" && typeof change.content === "string")
        .map((change) => ({ path: change.path, content: change.content, readOnly: true, candidate: true }));
      const targetWorkspace = {
        root: ticket.projectRoot,
        files: [...(target.operation === "create" ? [{ path: target.path }] : [target]), ...dependencyCandidates],
      };
      let coderResult = null;
      let lastCandidateError = null;
      for (let attempt = 1; attempt <= 2; attempt += 1) {
        const rejectionDetail = lastCandidateError?.message || "候选内容不完整";
        const retryInstruction = testRequirement
          ? `上一个候选被安全校验拒绝：${rejectionDetail}。重新生成完整的 ${target.path}。${testRequirement} 直接导入本轮候选实现，不要定义同名实现函数。只输出文件内容。`
          : `上一个候选被安全校验拒绝：${rejectionDetail}。只返回 ${target.path} 的完整文件内容；保持原语言以及全部既有接口，并确保明确要求的函数名、字符串和返回值都正确。`;
        const attemptMessages = attempt === 1 ? messages : [...messages, { role: "user", content: retryInstruction }];
        const generated = await this.activeCoder().generate({ task_id: ticket.id, messages: attemptMessages, workspace: targetWorkspace, tools: [], mode: "write_content", requested_tool: requestedTool });
        this.audit.append("coder.response", ticket.id, { route: "explicit_batch_edit", batchPath: target.path, attempt, ...responseAuditMetadata(generated) });
        if (generated.type !== "tool_request" || generated.tool?.name !== "write_file" || typeof generated.tool.arguments?.content !== "string") continue;
        generated.tool.arguments.path = target.path;
        generated.tool.arguments.content = normalizeCoderFileContent(generated.tool.arguments.content);
        const completedTestCandidate = completePythonTestImports(
          target.path,
          generated.tool.arguments.content,
          scopedRequest,
          relatedImplementations,
        );
        if (completedTestCandidate !== generated.tool.arguments.content) {
          generated.tool.arguments.content = completedTestCandidate;
          this.audit.append("coder.candidate_completed", ticket.id, {
            route: "explicit_batch_edit",
            path: target.path,
            attempt,
            strategy: "add_explicit_test_import",
          });
        }
        try {
          this.tools.validateReplacement(target.path, target.operation === "create" ? null : target.content, generated.tool.arguments.content);
          validateExplicitCandidateRequirements(target.path, generated.tool.arguments.content, scopedRequest, request);
          coderResult = generated;
          break;
        } catch (error) {
          lastCandidateError = error;
          this.audit.append("coder.candidate_rejected", ticket.id, { route: "explicit_batch_edit", path: target.path, attempt, code: error.code || "candidate_invalid", message: error.message });
        }
      }
      if (!coderResult) throw new AppError("coder_candidate_failed", `本地代码模型没有为 ${target.path} 生成完整且有效的候选内容，已安全停止，所有文件均未写入。`, 422);
      changes.push({ path: target.path, operation: target.operation, content: coderResult.tool.arguments.content });
    }

    const tool = { name: "write_files", arguments: { changes } };
    validateToolRequest(tool, this.tools.listFiles(ticket.projectRoot));
    for (const target of selected.filter((item) => item.operation !== "create")) {
      ticket.execution.messages.push(
        { role: "assistant", content: "", tool_calls: [{ name: "read_file", arguments: { path: target.path } }] },
        { role: "tool", content: target.content },
      );
    }
    ticket.execution.messages.push({ role: "assistant", content: "", tool_calls: [tool] });
    const toolResult = await this.callTool(ticket.id, tool.name, tool.arguments);
    ticket.execution.pendingEffectId = toolResult.effect.id;
    ticket.execution.directEditEffectId = toolResult.effect.id;
    this.audit.append("coder.deterministic_batch", ticket.id, { paths: changes.map((change) => change.path) });
    this.store.save();
    return {
      type: "message",
      content: `已读取 ${changes.length} 个目标文件并生成一个统一修改批次，正在等待你的批准。`,
      tool,
      effect: toolResult.effect,
      route: "explicit_batch_edit",
    };
  }

  async directGeneratedFile(ticket, request) {
    if (ticket.effects.some((effect) => effect.status === "pending") || !/(保存为|保存成|保存到|存为|存成|写到|写入|落盘|save\s+(?:as|to))/i.test(request)) return null;
    const previous = [...ticket.execution.messages]
      .reverse()
      .find((message) => message.role === "assistant" && /```/.test(message.content || ""));
    if (!previous) return null;
    const block = extractCodeBlock(previous.content);
    if (!block?.content.trim()) return null;
    const outputPath = inferGeneratedFilePath(request, block, ticket.execution.messages);
    if (!outputPath) {
      const result = { type: "needs_clarification", route: "save_generated_file", content: "请告诉我要保存的文件名，例如 `knapsack.py`。我会先展示新文件 Diff，批准后才写入。", tool: null };
      report(ticket, ReportKind.WAITING, result);
      this.store.save();
      return result;
    }
    const tool = { name: "write_file", arguments: { path: outputPath, content: `${block.content.trimEnd()}\n` } };
    const toolResult = await this.callTool(ticket.id, tool.name, tool.arguments);
    ticket.execution.messages.push({ role: "assistant", content: "", tool_calls: [tool] });
    ticket.execution.pendingEffectId = toolResult.effect.id;
    ticket.execution.directEditEffectId = toolResult.effect.id;
    this.store.save();
    return {
      type: "message",
      content: `已从上一条回答生成 \`${outputPath}\` 的新文件提案。请检查 Diff，批准后才会写入当前项目。`,
      tool,
      effect: toolResult.effect,
      route: "save_generated_file",
    };
  }

  workspaceContext(ticket, request, { includeContent = true } = {}) {
    const index = this.tools.listFiles(ticket.projectRoot);
    const normalizedRequest = String(request || "").replaceAll("\\", "/").toLowerCase();
    const selected = includeContent
      ? index.filter((file) => normalizedRequest.includes(file.toLowerCase()) || normalizedRequest.includes(path.basename(file).toLowerCase())).slice(0, 20)
      : [];
    const selectedSet = new Set(selected);
    const files = index.map((file) => {
      if (!selectedSet.has(file)) return { path: file };
      try { return { path: file, content: this.tools.readFile(ticket.projectRoot, { path: file }).content }; }
      catch { return { path: file }; }
    });
    const attachments = (this.activeAttachmentContexts.get(ticket.id)?.attachments || []).map((attachment) => ({
      path: attachmentVirtualPath(attachment),
      content: attachment.content,
      attachment: true,
      displayName: attachment.displayName,
      source: attachment.source,
      sha256: attachment.sha256,
      truncated: attachment.truncated,
    }));
    return { root: ticket.projectRoot, files: [...files, ...attachments] };
  }

  async callTool(id, name, args = {}) {
    const ticket = this.ticket(id);
    const policy = assessTool(name);
    if (["write_file", "write_files"].includes(name)) {
      const effect = name === "write_files"
        ? this.tools.prepareBatch(ticket.projectRoot, args)
        : this.tools.prepareWrite(ticket.projectRoot, args);
      ticket.effects.push(effect);
      ticket.execution.testsPassed = false;
      report(ticket, ReportKind.WAITING, { reason: "approval", effectId: effect.id });
      this.audit.append("effect.proposed", id, {
        effectId: effect.id,
        paths: effect.changes.map((change) => change.path),
        changes: effect.changes.map((change) => ({ path: change.path, operation: change.operation, afterDigest: change.afterDigest })),
      });
      this.store.save();
      return { policy, effect };
    }
    const startedAt = new Date().toISOString();
    const result = await this.tools[name === "read_file" ? "readFile" : name](ticket.projectRoot, args);
    const evidence = this.recordEvidence(ticket, name, args, result, startedAt);
    return { policy, result, evidence };
  }

  approve(id, effectId, approved, selectedPaths = null) {
    const ticket = this.ticket(id);
    const effect = ticket.effects.find((item) => item.id === effectId);
    if (!effect) throw new AppError("effect_not_found", "Effect not found", 404);
    if (effect.status !== "pending") throw new AppError("effect_resolved", "Effect was already resolved", 409);
    let result;
    if (approved) {
      effect.status = "approved";
      requireApproval(effect);
      try {
        result = this.tools.applyBatch(ticket.projectRoot, effect, selectedPaths);
      } catch (error) {
        if (error.code === "batch_recovery_failed") {
          effect.status = "recovery_required";
          ticket.execution.pendingEffectId = null;
          ticket.state = TicketState.FAILED;
          this.audit.append("effect.recovery_required", id, { effectId, message: error.message });
        } else {
          effect.status = "pending";
        }
        this.store.save();
        throw error;
      }
      this.audit.append("effect.approved", id, { effectId, selectedPaths: result.paths });
      ticket.state = TicketState.OPEN;
      this.audit.append("effect.applied", id, result);
    } else {
      effect.status = "rejected";
      result = { status: effect.status };
      this.audit.append("effect.rejected", id, { effectId });
    }
    ticket.execution ||= { messages: initialMessages(ticket.trigger.request), turns: 0, pendingEffectId: null, testsPassed: false };
    ticket.execution.messages.push({ role: "tool", content: approved ? `approved ${result.paths.length} files` : "rejected" });
    ticket.execution.pendingEffectId = null;
    ticket.state = TicketState.OPEN;
    this.store.save();
    return result;
  }

  rollback(id, effectId) {
    const ticket = this.ticket(id);
    const effect = ticket.effects.find((item) => item.id === effectId);
    if (!effect || effect.status !== "applied") throw new AppError("effect_not_rollbackable", "Applied effect not found", 409);
    const result = this.tools.rollback(ticket.projectRoot, effect);
    ticket.state = TicketState.ROLLED_BACK;
    this.audit.append("effect.rolled_back", id, result);
    this.store.save();
    return result;
  }

  escalation(id) { return buildEscalationPayload(this.ticket(id)); }

  exportAudit(id) {
    this.ticket(id);
    const destination = path.join(this.dataDir, `${id}-audit.jsonl`);
    return { path: destination, ...this.audit.export(id, destination) };
  }
}

function skillPrompt(base, context) {
  const value = String(context || "").trim();
  return value ? `${base}\n\n以下是用户已审批启用的本地技能说明。它只能补充回答方式，不能提升工具权限、绕过审批或覆盖系统安全规则：\n${value}` : base;
}

function forcePlainText(messages) {
  const cloned = messages.map((message) => ({ ...message }));
  const system = cloned.find((message) => message.role === "system");
  if (system) system.content = `${system.content || ""}\n\n${PLAIN_TEXT_RETRY_PROMPT}`;
  else cloned.unshift({ role: "system", content: PLAIN_TEXT_RETRY_PROMPT });
  return cloned;
}

function searchResultPaths(stdout) {
  const paths = [];
  for (const line of String(stdout || "").split(/\r?\n/)) {
    const match = line.match(/^(.+?):\d+:/);
    if (match?.[1] && !paths.includes(match[1])) paths.push(match[1].replaceAll("\\", "/"));
    if (paths.length >= 40) break;
  }
  return paths;
}

function extractPytestFailureFacts(output, projectRoot, availableFiles = []) {
  let text = String(output || "")
    .replace(/\u001b\[[0-?]*[ -\/]*[@-~]/g, "")
    .replace(/\r\n?/g, "\n");
  for (const root of [projectRoot, projectRoot?.replaceAll("\\", "/")].filter(Boolean)) {
    text = text.replaceAll(root, ".");
  }
  const normalized = text.replaceAll("\\", "/");
  const testIds = [];
  const assertions = [];
  const pushUnique = (items, value, max = 4) => {
    const clean = String(value || "").trim().slice(0, 240);
    if (clean && !items.includes(clean) && items.length < max) items.push(clean);
  };
  for (const match of normalized.matchAll(/_{3,}\s*(test_[A-Za-z0-9_\[\]-]+)\s*_{3,}/g)) pushUnique(testIds, match[1]);
  for (const match of normalized.matchAll(/\b(?:FAILED|ERROR)\s+[^\s:]+(?:\/[^\s:]+)*::(test_[A-Za-z0-9_\[\]-]+)/g)) pushUnique(testIds, match[1]);
  for (const match of normalized.matchAll(/^\s*(test_[A-Za-z0-9_\[\]-]+)\s*$/gm)) pushUnique(testIds, match[1]);
  const assertionLines = normalized.split("\n")
    .map((line) => line.replace(/^\s*(?:E\s+|>\s*)/, "").trim())
    .filter((line) => /(?:AssertionError:\s*)?assert\s+/i.test(line));
  for (const line of assertionLines) pushUnique(assertions, line);
  const concrete = assertionLines.find((line) => /^AssertionError:\s*assert\s+/i.test(line)) || assertionLines[0] || "";
  const comparisonMatch = concrete.match(/(?:AssertionError:\s*)?assert\s+(.+?)\s+(==|!=|<=|>=|<|>)\s+(.+)$/i);
  const comparison = comparisonMatch ? {
    actual: comparisonMatch[1].trim().slice(0, 180),
    operator: comparisonMatch[2],
    expected: comparisonMatch[3].trim().slice(0, 180),
  } : null;
  const indexedTests = availableFiles
    .map((file) => String(file).replaceAll("\\", "/").replace(/^\.\//, ""))
    .filter((file) => /(?:^|\/)tests?(?:\/|$)/i.test(file) || /(?:^|\/)test_[^/]+\.py$/i.test(file));
  const testPaths = indexedTests.filter((file) => normalized.includes(file)).slice(0, 3);
  return { testIds, assertions, comparison, testPaths };
}

function formatPytestFailureFacts(facts = {}) {
  const lines = [];
  if (facts.testIds?.length) lines.push(`失败用例：${facts.testIds.join("、")}`);
  if (facts.testPaths?.length) lines.push(`已证明的测试文件（只读）：${facts.testPaths.join("、")}`);
  for (const assertion of (facts.assertions || []).slice(0, 3)) lines.push(`失败断言：${assertion}`);
  if (facts.comparison) {
    lines.push(`实际值：${facts.comparison.actual}`);
    lines.push(`比较符：${facts.comparison.operator}`);
    lines.push(`期望值：${facts.comparison.expected}`);
  }
  return lines.length ? lines.join("\n") : "未能从输出中可靠抽取更多结构化事实。";
}

function requestsPytest(request) {
  return /(^|\W)pytest(\W|$)|运行.{0,8}测试|执行.{0,8}测试|run\s+(?:the\s+)?tests?/i.test(String(request || ""));
}

function isTestRepairFollowUp(request) {
  const text = String(request || "").trim();
  return /(?:继续|接着|根据|按照|结合|用).{0,20}(?:修复|处理|修改|改)|(?:修复|处理).{0,20}(?:失败|测试|pytest)|fix.{0,20}(?:failure|failed|test)/i.test(text);
}

function requestsMutation(request) {
  return /(修复|修改|改成|实现|新增|添加|增加|删除|移除|重构|写入|创建|新建|保存|落盘|fix|modify|implement|refactor|add|delete|remove|create|write|save)/i.test(String(request || ""));
}

function gateToolTaskCompletion(ticket, request, result) {
  if (result.type === "error") return result;
  const evidence = ticket.execution.evidence || [];
  const hasProjectEvidence = evidence.some((item) => ["read_file", "search", "pytest"].includes(item.toolName) && item.status === "succeeded");
  const pytestEvidence = [...evidence].reverse().find((item) => item.toolName === "pytest");
  const hasAppliedEffect = ticket.effects.some((effect) => effect.status === "applied");
  const hasPendingEffect = ticket.effects.some((effect) => effect.status === "pending");
  if (requestsMutation(request) && !hasAppliedEffect && !hasPendingEffect) {
    return {
      type: "needs_clarification",
      route: Intent.TOOL_TASK,
      content: "我还没有生成可审查的修改 Diff，因此没有把任务标记为完成。请指定要修改的文件和预期结果。",
      evidence,
    };
  }
  if (requestsPytest(request)) {
    if (!pytestEvidence) {
      return {
        type: "needs_clarification",
        route: Intent.TOOL_TASK,
        content: "我还没有取得真实测试结果，因此没有把任务标记为完成。请指定测试文件或允许我运行 pytest。",
        evidence,
      };
    }
    if (pytestEvidence.exitCode !== 0) {
      return {
        type: "needs_clarification",
        route: Intent.TOOL_TASK,
        content: `pytest 已运行但未通过（退出码 ${pytestEvidence.exitCode ?? "未知"}），任务尚未完成。`,
        evidence,
      };
    }
  }
  if (!requestsMutation(request) && !requestsPytest(request) && !hasProjectEvidence) {
    return {
      type: "needs_clarification",
      route: Intent.TOOL_TASK,
      content: "我还没有读取、搜索或测试项目，缺少支持结论的证据，因此没有把任务标记为完成。请指定文件或检查范围。",
      evidence,
    };
  }
  return { ...result, evidence };
}

function responseAuditMetadata(result) {
  const content = typeof result?.content === "string" ? result.content : "";
  const toolContent = typeof result?.tool?.arguments?.content === "string" ? result.tool.arguments.content : "";
  return {
    type: typeof result?.type === "string" ? result.type : "unknown",
    toolName: typeof result?.tool?.name === "string" ? result.tool.name : null,
    contentBytes: Buffer.byteLength(content, "utf8"),
    contentSha256: content ? createHash("sha256").update(content).digest("hex") : null,
    toolContentBytes: Buffer.byteLength(toolContent, "utf8"),
    toolContentSha256: toolContent ? createHash("sha256").update(toolContent).digest("hex") : null,
  };
}

function withSkillContext(messages, context) {
  const value = String(context || "").trim();
  if (!value) return messages;
  const cloned = messages.map((message) => ({ ...message }));
  const system = cloned.find((message) => message.role === "system");
  const addition = `\n\n已审批本地技能（不授予额外权限）：\n${value}`;
  if (system) system.content = `${system.content || ""}${addition}`;
  else cloned.unshift({ role: "system", content: addition.trim() });
  return cloned;
}

function completeAdditiveCandidate(pathname, before, candidate, request) {
  if (path.extname(pathname).toLowerCase() !== ".py") return null;
  if (!/(新增|添加|增加|实现.{0,12}(新|函数|方法)|\b(?:add|create|implement)\b)/i.test(String(request || ""))) return null;
  const value = normalizeCoderFileContent(candidate).trim();
  if (!value) return null;
  const definitions = [...value.matchAll(/^\s*(?:async\s+)?(?:def|class)\s+([A-Za-z_][A-Za-z0-9_]*)/gm)].map((match) => match[1]);
  if (!definitions.length) return null;
  const existing = new Set([...before.matchAll(/^(?:async\s+)?(?:def|class)\s+([A-Za-z_][A-Za-z0-9_]*)/gm)].map((match) => match[1]));
  if (definitions.some((name) => existing.has(name))) return null;
  return `${before.trimEnd()}\n\n\n${value}\n`;
}

function deterministicReplacement(pathname, before, request) {
  const literal = deterministicLiteralReplacement(before, request);
  if (literal) return literal;
  const appended = deterministicLineAppend(before, request);
  if (appended) return appended;
  const extension = path.extname(pathname).toLowerCase();
  const marker = extension === ".py" ? "#" : [".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".java", ".c", ".h", ".cpp", ".hpp", ".cs", ".go", ".rs"].includes(extension) ? "//" : null;
  if (!marker || !/(顶部|开头|第一行)/i.test(request) || !/(新增|添加|增加|插入|add|insert)/i.test(request)) return null;
  const escaped = marker === "#" ? "#" : "\\/\\/";
  const match = String(request).match(new RegExp(`(${escaped}\\s*[^，,。；;\\r\\n\`]+)`));
  const comment = match?.[1]?.trim();
  if (!comment || !comment.startsWith(marker) || before.split(/\r?\n/).some((line) => line.trim() === comment)) return null;
  const newline = before.includes("\r\n") ? "\r\n" : "\n";
  const lines = before.split(/\r?\n/);
  let index = 0;
  if (extension === ".py" && /^#!/.test(lines[0] || "")) index = 1;
  if (extension === ".py" && /^#.*coding[:=]/i.test(lines[index] || "")) index += 1;
  lines.splice(index, 0, comment);
  return lines.join(newline);
}

function deterministicLiteralReplacement(before, request) {
  const text = String(request || "");
  const match = text.match(/(?:从|\bfrom\b)\s*[`'"“]?(.+?)[`'"”]?\s*(?:改为|改成|替换为|换成|\bto\b)\s*[`'"“]?(.+?)[`'"”]?(?=$|[，,。；;\r\n])/i);
  if (!match) return null;
  const oldValue = trimReplacementToken(match[1]);
  const newValue = trimReplacementToken(match[2]);
  if (!oldValue || !newValue || oldValue === newValue) return null;
  if (before.split(oldValue).length - 1 !== 1 || before.includes(newValue)) return null;
  return before.replace(oldValue, newValue);
}

function deterministicLineAppend(before, request) {
  const text = String(request || "");
  if (!/(末尾|结尾|end)/i.test(text) || !/(增加|添加|追加|append)/i.test(text)) return null;
  const match = text.match(/[`"“]([^`"”\r\n]{2,240})[`"”]/);
  const line = match?.[1]?.trim();
  if (!line || before.split(/\r?\n/).some((existing) => existing.trim() === line)) return null;
  const newline = before.includes("\r\n") ? "\r\n" : "\n";
  return `${before.trimEnd()}${newline}${newline}${line}${newline}`;
}

function trimReplacementToken(value) {
  return String(value || "")
    .trim()
    .replace(/^[`'"“]+|[`'"”]+$/g, "")
    .trim();
}

function normalizeCoderFileContent(candidate) {
  return String(candidate || "")
    .trim()
    .replace(/^Tool result:\s*/i, "")
    .replace(/^\[write_file\]\s+\S+\s+/i, "")
    .replace(/^```(?:[A-Za-z0-9_+-]+)?\s*\r?\n/, "")
    .replace(/\r?\n```\s*$/, "")
    .trimEnd()
    .concat("\n");
}

function validateRepairCodeChange(pathname, before, candidate) {
  if (repairCodeProjection(pathname, before) === repairCodeProjection(pathname, candidate)) {
    throw new AppError("candidate_no_effect", "代码模型只改变了注释或格式，没有修改可执行实现");
  }
}

function repairCodeProjection(pathname, content) {
  const extension = path.extname(pathname).toLowerCase();
  const normalized = String(content || "").replace(/^\uFEFF/, "").replace(/\r\n/g, "\n");
  if (extension !== ".py") return normalized.split("\n").map((line) => line.trimEnd()).filter((line) => line.trim() && !/^\s*(?:\/\/|\/\*|\*|\*\/)/.test(line)).join("\n");
  const projected = [];
  let triple = null;
  for (const rawLine of normalized.split("\n")) {
    const trimmed = rawLine.trim();
    if (triple) {
      if (trimmed.includes(triple)) triple = null;
      continue;
    }
    const opener = trimmed.startsWith('"""') ? '"""' : trimmed.startsWith("'''") ? "'''" : null;
    if (opener) {
      if (trimmed.indexOf(opener, opener.length) < 0) triple = opener;
      continue;
    }
    const code = pythonLineWithoutComment(rawLine).trimEnd();
    if (code.trim()) projected.push(code);
  }
  return projected.join("\n");
}

function pythonLineWithoutComment(line) {
  let quote = null;
  let escaped = false;
  for (let index = 0; index < line.length; index += 1) {
    const character = line[index];
    if (escaped) { escaped = false; continue; }
    if (character === "\\") { escaped = true; continue; }
    if (quote) {
      if (character === quote) quote = null;
      continue;
    }
    if (character === '"' || character === "'") { quote = character; continue; }
    if (character === "#") return line.slice(0, index);
  }
  return line;
}

function extractCodeBlock(content) {
  const match = String(content || "").match(/```([A-Za-z0-9_+-]*)\s*\r?\n([\s\S]*?)```/);
  return match ? { language: match[1].toLowerCase(), content: match[2] } : null;
}

function inferGeneratedFilePath(request, block, messages) {
  const explicit = String(request || "").match(/(?:^|[\s`"'（(])([A-Za-z0-9][A-Za-z0-9._-]{0,79}\.(?:py|js|ts|tsx|jsx|rs|go|java|cpp|c|cs|html|css|sql|sh|ps1|md|json))(?:$|[\s`"'，。；、）)])/i);
  if (explicit) return explicit[1];
  const extension = ({ python: "py", py: "py", javascript: "js", js: "js", typescript: "ts", ts: "ts", rust: "rs", go: "go", java: "java", cpp: "cpp", c: "c", csharp: "cs", html: "html", css: "css", sql: "sql", bash: "sh", shell: "sh", powershell: "ps1", json: "json" })[block.language];
  const requestedExtension = String(request || "").match(/\.(py|js|ts|tsx|jsx|rs|go|java|cpp|c|cs|html|css|sql|sh|ps1|md|json)\b/i)?.[1].toLowerCase();
  const suffix = requestedExtension || extension;
  if (!suffix) return null;
  const definition = block.content.match(/^\s*(?:async\s+)?(?:def|function|fn)\s+([A-Za-z_][A-Za-z0-9_]*)/m)?.[1];
  let stem = definition?.replace(/(?:_\d+)+$/, "").replace(/^main$/, "") || "";
  const priorText = messages.filter((message) => message.role === "user").map((message) => message.content).join("\n");
  if (!stem && /背包/.test(priorText)) stem = "knapsack";
  if (!stem) stem = "generated";
  return `${stem}.${suffix}`;
}

function asksAboutCurrentAttachments(request) {
  const text = String(request || "").trim();
  return /(本轮|当前).{0,12}(是否|有无|还有|仍有).{0,8}附件|(本轮|当前).{0,8}附件.{0,8}(哪些|文件名|列表|还在|存在)|是否.{0,8}(有|仍有).{0,8}附件/i.test(text);
}

function groundProjectAnswer(content, workspace) {
  const selected = workspace.files.filter((file) => typeof file.content === "string");
  if (!selected.length) return content;
  const normalized = content.replaceAll("\\n", "\n");
  const source = selected.map((file) => file.content).join("\n");
  const codeClaims = normalized.split(/\r?\n/).map((line) => line.trim()).filter((line) => /^(def |class |return |from |import )/.test(line));
  const contradicted = codeClaims.some((line) => !source.includes(line));
  if (contradicted) return withSources("本地模型的解释未通过文件内容一致性检查，因此没有展示可能错误的结论。", selected.map(sourceLabel));
  return withSources(normalized, selected.map(sourceLabel));
}

function normalizeAttachments(value) {
  if (!Array.isArray(value)) throw new AppError("invalid_request", "Attachments must be an array");
  if (value.length > 8) throw new AppError("attachment_limit", "A turn accepts at most 8 attachments", 413);
  let total = 0;
  return value.map((item) => {
    if (!item || typeof item !== "object") throw new AppError("attachment_invalid", "Attachment metadata is invalid", 422);
    const id = String(item.id || "");
    const displayName = String(item.displayName || "");
    const source = String(item.source || "");
    const relativePath = item.relativePath == null ? null : String(item.relativePath).replaceAll("\\", "/");
    const content = typeof item.content === "string" ? item.content : "";
    const contentBytes = Buffer.byteLength(content, "utf8");
    const sizeBytes = Number(item.sizeBytes);
    const includedBytes = Number(item.includedBytes);
    const sha256 = String(item.sha256 || "");
    const contentSha256 = String(item.contentSha256 || "");
    if (!/^attachment-[0-9a-f-]{36}$/i.test(id) || !displayName || /[\\/\0\r\n]/.test(displayName)) {
      throw new AppError("attachment_invalid", "Attachment identity is invalid", 422);
    }
    if (!["project", "external_readonly"].includes(source)) throw new AppError("attachment_invalid", "Attachment source is invalid", 422);
    if (source === "project" && (!relativePath || path.isAbsolute(relativePath) || relativePath.split("/").includes(".."))) {
      throw new AppError("attachment_invalid", "Project attachment path is invalid", 422);
    }
    if (source === "external_readonly" && relativePath !== null) throw new AppError("attachment_invalid", "External attachments cannot expose a path", 422);
    if (!Number.isSafeInteger(sizeBytes) || sizeBytes < 1 || sizeBytes > 256 * 1024 || !Number.isSafeInteger(includedBytes) || includedBytes !== contentBytes) {
      throw new AppError("attachment_invalid", "Attachment size metadata is invalid", 422);
    }
    if (!/^[0-9a-f]{64}$/i.test(sha256) || createHash("sha256").update(content).digest("hex") !== contentSha256) {
      throw new AppError("attachment_digest_mismatch", "Attachment content digest does not match", 422);
    }
    total += contentBytes;
    if (total > 120_000) throw new AppError("attachment_context_too_large", "Attachment context exceeds 120,000 bytes", 413);
    return { id, displayName, source, relativePath, sizeBytes, includedBytes, truncated: Boolean(item.truncated), sha256, contentSha256, content };
  });
}

function attachmentVirtualPath(attachment) {
  return attachment.source === "project"
    ? `attachment://project/${attachment.relativePath}`
    : `attachment://external/${attachment.id}/${attachment.displayName}`;
}

function attachmentAuditMetadata(attachment) {
  return {
    id: attachment.id,
    displayName: attachment.displayName,
    source: attachment.source,
    relativePath: attachment.relativePath,
    sizeBytes: attachment.sizeBytes,
    includedBytes: attachment.includedBytes,
    truncated: attachment.truncated,
    sha256: attachment.sha256,
  };
}

function attachmentEvidence(attachment) {
  const at = new Date().toISOString();
  return {
    toolName: "attachment_read",
    arguments: { id: attachment.id, displayName: attachment.displayName, source: attachment.source },
    startedAt: at,
    finishedAt: at,
    status: "succeeded",
    sourcePaths: [attachmentVirtualPath(attachment)],
    contentDigest: attachment.sha256,
    preview: null,
    error: null,
  };
}

function sourceLabel(file) {
  if (!file.attachment) return file.path;
  const prefix = file.source === "external_readonly" ? "外部只读附件" : "项目附件";
  return `${prefix}：${file.displayName}`;
}

function resolveRequestedFile(request, index, previousPaths) {
  const normalizedRequest = String(request || "").replaceAll("\\", "/").toLowerCase();
  if (isReadFollowUp(request) && previousPaths.length === 1 && index.includes(previousPaths[0])) {
    return { status: "resolved", path: previousPaths[0], reason: "conversation_context" };
  }
  const fullMatches = index.filter((file) => normalizedRequest.includes(file.toLowerCase()));
  if (fullMatches.length === 1) return { status: "resolved", path: fullMatches[0], reason: "relative_path" };
  const baseMatches = index.filter((file) => normalizedRequest.includes(path.basename(file).toLowerCase()));
  if (baseMatches.length === 1) return { status: "resolved", path: baseMatches[0], reason: "file_name" };
  if (baseMatches.length > 1) return { status: "ambiguous", candidates: baseMatches };
  if (/(随便|任意|任选|挑一个)/i.test(request)) {
    const stable = index.filter(isReadableText).sort((left, right) => stableFileScore(left) - stableFileScore(right) || left.localeCompare(right))[0];
    if (stable) return { status: "resolved", path: stable, reason: "stable_readable_choice" };
  }
  const requested = extractRequestedPath(request);
  return { status: "missing", candidates: [], requested };
}

function extractRequestedPath(request) {
  return extractRequestedPaths(request)[0] || null;
}

function extractRequestedPaths(request) {
  const normalized = String(request || "").replaceAll("\\", "/");
  return [...normalized.matchAll(/(?:^|[\s`'"“”‘’（(、，和与及])([\w.@+-]+(?:\/[\w.@+ -]+)*\.[A-Za-z0-9]{1,12})(?=$|的|[\s`'"“”‘’）),，。！？!?：:；;、和与及])/gu)].map((match) => match[1]);
}

export function resolveExplicitBatchTargets(index, request) {
  const requested = [...new Set(extractRequestedPaths(request).map((item) => item.replaceAll("\\", "/")))]
    .filter((requestedPath) => !requestNegatesPath(request, requestedPath));
  if (requested.length < 2 || requested.length > 20) return null;
  const normalizedIndex = index.map((item) => ({ path: item, lower: item.toLowerCase(), base: path.basename(item).toLowerCase() }));
  const targets = [];
  for (const requestedPath of requested) {
    const lower = requestedPath.toLowerCase();
    const exact = normalizedIndex.find((item) => item.lower === lower);
    const basenameMatches = normalizedIndex.filter((item) => item.base === lower);
    const existing = exact || (basenameMatches.length === 1 ? basenameMatches[0] : null);
    if (!exact && basenameMatches.length > 1) return null;
    const clause = requestClauseForPath(request, requestedPath);
    const deleting = /(删除|移除|删掉|清除|\bdelete\b|\bremove\b)/i.test(clause);
    const creating = /(新建|创建|新增|生成|\bcreate\b|\badd\b)/i.test(clause);
    const modifying = /(修复|修改|改成|改为|替换|重构|更新|增加|添加|追加|插入|\bfix\b|\bmodify\b|\brefactor\b|\bupdate\b|\breplace\b|\bappend\b|\binsert\b)/i.test(clause);
    if (deleting) {
      if (!existing) return null;
      targets.push({ path: existing.path, operation: "delete" });
    } else if (modifying && existing) {
      targets.push({ path: existing.path, operation: "modify" });
    } else if (creating) {
      if (existing) return null;
      targets.push({ path: requestedPath, operation: "create" });
    } else if (modifying) {
      if (!existing) return null;
      targets.push({ path: existing.path, operation: "modify" });
    } else {
      return null;
    }
  }
  return targets;
}

export function requestNegatesPath(request, requestedPath) {
  const escaped = requestedPath.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const action = "(?:读取|修改|改动|写入|删除|移除|创建|新建|处理|触碰|更改)?";
  return new RegExp(`(?:不要|不能|不得|禁止|无需|不应|勿)\\s*${action}[^；;。\\r\\n]{0,16}${escaped}`, "i")
    .test(String(request || "").replaceAll("\\", "/"));
}

function requestClauseForPath(request, requestedPath) {
  const value = String(request || "");
  const lower = value.toLowerCase();
  const at = lower.indexOf(requestedPath.toLowerCase());
  if (at < 0) return value;
  const before = Math.max(value.lastIndexOf("；", at), value.lastIndexOf(";", at), value.lastIndexOf("。", at));
  const endings = [value.indexOf("；", at), value.indexOf(";", at), value.indexOf("。", at)].filter((item) => item >= 0);
  const after = endings.length ? Math.min(...endings) : value.length;
  return value.slice(before + 1, after);
}

function scopedRequestForTarget(request, target) {
  const value = String(request || "");
  const base = path.basename(target.path).toLowerCase();
  const stemParts = path.basename(target.path, path.extname(target.path)).toLowerCase().split(/[^a-z0-9]+/).filter((item) => item.length >= 3 && !["file", "batch", "mixed", "test"].includes(item));
  const definitions = typeof target.content === "string"
    ? [...target.content.matchAll(/^(?:\s*(?:export\s+)?(?:default\s+)?)?(?:async\s+)?(?:def|class|function)\s+([A-Za-z_$][A-Za-z0-9_$]*)/gm)].map((match) => match[1].toLowerCase())
    : [];
  const segments = value.split(/[；;。，,\r\n]+/).map((item) => item.trim()).filter(Boolean);
  const exactAnchors = [...new Set([target.path.toLowerCase(), base])];
  const exactIndexes = segments.flatMap((segment, index) => {
    const lower = segment.toLowerCase();
    return exactAnchors.some((anchor) => lower.includes(anchor)) ? [index] : [];
  });
  if (exactIndexes.length) {
    const selectedIndexes = new Set(exactIndexes);
    for (const index of exactIndexes) {
      for (let next = index + 1; next < segments.length; next += 1) {
        const continuation = segments[next];
        if (extractRequestedPaths(continuation).length) break;
        if (!/^(?:返回|并|同时|其|值|内容|要求|其中|包含|覆盖|测试|验证|断言|检查|return\b|assert\b)/i.test(continuation)) break;
        selectedIndexes.add(next);
      }
    }
    return [...selectedIndexes].sort((left, right) => left - right).map((index) => segments[index]).join("；");
  }
  const fallbackAnchors = [...new Set([...stemParts, ...definitions])];
  const selected = segments.filter((segment) => fallbackAnchors.some((anchor) => segment.toLowerCase().includes(anchor)));
  return selected.length ? selected.join("；") : requestClauseForPath(value, target.path);
}

function validateExplicitCandidateRequirements(pathname, content, requestClause, fullRequest = requestClause) {
  const tokens = new Set();
  for (const match of String(requestClause || "").matchAll(/\b([A-Za-z_][A-Za-z0-9_]*)\s*\(\s*\)/g)) tokens.add(match[1]);
  for (const match of String(requestClause || "").matchAll(/[`"']([^`"'\r\n]{2,80})[`"']/g)) tokens.add(match[1]);
  for (const match of String(requestClause || "").matchAll(/返回(?:值)?(?:改成|为)?\s*([A-Za-z0-9_.-]{2,80})/gi)) tokens.add(match[1]);
  const missing = [...tokens].filter((token) => !content.includes(token));
  if (missing.length) throw new AppError("candidate_requirement_missing", `代码模型生成的 ${pathname} 缺少明确要求：${missing.join("、")}`);
  if (/(?:^|\/)test_[^/]+\.py$/i.test(pathname) || /(?:^|\/)tests?\/[^/]+\.py$/i.test(pathname)) {
    if (!/^\s*(?:async\s+)?def\s+test_[A-Za-z0-9_]+\s*\(/m.test(content) || !/\bassert\b/.test(content)) {
      throw new AppError("candidate_test_missing", `代码模型生成的 ${pathname} 不包含可由 pytest 收集的 test_ 函数和断言`);
    }
    const requiredCalls = [...String(requestClause || "").matchAll(/\b([A-Za-z_][A-Za-z0-9_]*)\s*\(\s*([^)]{1,60})\s*\)/g)]
      .map((match) => `${match[1]}(${match[2].replace(/\s+/g, "")})`)
      .filter((call) => !/^pytest\(/i.test(call));
    const compact = content.replace(/\s+/g, "");
    const missingCalls = [...new Set(requiredCalls)].filter((call) => !compact.includes(call));
    if (missingCalls.length) throw new AppError("candidate_test_cases_missing", `代码模型生成的 ${pathname} 缺少测试用例：${missingCalls.join("、")}`);
    const related = relatedImplementationPaths(fullRequest, pathname);
    for (const implementationPath of related) {
      const moduleName = path.basename(implementationPath, ".py");
      const imported = new RegExp(`(?:^|\\n)\\s*(?:from\\s+${escapeRegExp(moduleName)}\\s+import\\s+|import\\s+${escapeRegExp(moduleName)}(?:\\s|$))`, "m").test(content);
      if (!imported) throw new AppError("candidate_test_import_missing", `代码模型生成的 ${pathname} 没有导入被测模块 ${implementationPath}`);
    }
    const invokedNames = [...String(requestClause || "").matchAll(/\b([A-Za-z_][A-Za-z0-9_]*)\s*\(/g)]
      .map((match) => match[1])
      .filter((name) => name !== "pytest" && !name.startsWith("test_"));
    const duplicated = [...new Set(invokedNames)].filter((name) => new RegExp(`^\\s*(?:async\\s+)?def\\s+${escapeRegExp(name)}\\s*\\(`, "m").test(content));
    if (related.length && duplicated.length) throw new AppError("candidate_test_duplicates_implementation", `代码模型生成的 ${pathname} 复制了被测实现：${duplicated.join("、")}`);
  }
}

function completePythonTestImports(pathname, content, requestClause, relatedImplementations) {
  if (!/(?:^|\/)test_[^/]+\.py$/i.test(pathname) && !/(?:^|\/)tests?\/[^/]+\.py$/i.test(pathname)) return content;
  if (relatedImplementations.length !== 1) return content;
  if (!/^\s*(?:async\s+)?def\s+test_[A-Za-z0-9_]+\s*\(/m.test(content) || !/\bassert\b/.test(content)) return content;
  const implementationPath = relatedImplementations[0];
  const moduleName = path.basename(implementationPath, ".py");
  const alreadyImported = new RegExp(`(?:^|\\n)\\s*(?:from\\s+${escapeRegExp(moduleName)}\\s+import\\s+|import\\s+${escapeRegExp(moduleName)}(?:\\s|$))`, "m").test(content);
  if (alreadyImported) return content;
  const invokedNames = [...String(requestClause || "").matchAll(/\b([A-Za-z_][A-Za-z0-9_]*)\s*\(/g)]
    .map((match) => match[1])
    .filter((name) => name !== "pytest" && !name.startsWith("test_") && new RegExp(`\\b${escapeRegExp(name)}\\s*\\(`).test(content));
  const importedNames = [...new Set(invokedNames)].filter((name) => !new RegExp(`^\\s*(?:async\\s+)?def\\s+${escapeRegExp(name)}\\s*\\(`, "m").test(content));
  if (!importedNames.length) return content;
  return `from ${moduleName} import ${importedNames.join(", ")}\n\n${String(content).replace(/^\s+/, "")}`;
}

function relatedImplementationPaths(request, testPath) {
  if (!/(?:^|\/)test_[^/]+\.py$/i.test(testPath) && !/(?:^|\/)tests?\/[^/]+\.py$/i.test(testPath)) return [];
  return [...new Set(extractRequestedPaths(String(request || ""))
    .map((pathname) => pathname.replaceAll("\\", "/"))
    .filter((pathname) => pathname.toLowerCase().endsWith(".py") && pathname.toLowerCase() !== testPath.toLowerCase() && !/(?:^|\/)test_[^/]+\.py$/i.test(pathname)))];
}

function escapeRegExp(value) {
  return String(value).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function pytestPathsForEffect(effect) {
  return (effect?.changes || [])
    .map((change) => change?.path)
    .filter((pathname) => typeof pathname === "string" && (/(?:^|\/)test_[^/]+\.py$/i.test(pathname) || /(?:^|\/)tests?\/[^/]+\.py$/i.test(pathname)));
}

function isReadFollowUp(request) {
  return /^(它|这个文件|刚才的文件).{0,12}(讲|内容|作用|做什么|解释|总结)|^(详细说说|展开说说)[？?。.！!\s]*$/i.test(String(request || "").trim());
}

function isProjectQuestionFollowUp(request) {
  return /^(继续|接着|为什么|详细一点|再说说|展开说说|它|这个文件|刚才的文件|continue|why|more)[？?。.！!\s]*$/i.test(String(request || "").trim());
}

function isReadableText(file) {
  return TEXT_EXTENSIONS.has(path.extname(file).toLowerCase()) && !/(^|\/)(package-lock|pnpm-lock|yarn\.lock|cargo\.lock)(\/|$)/i.test(file);
}

function stableFileScore(file) {
  const base = path.basename(file).toLowerCase();
  if (/^readme(\.|$)/.test(base)) return 0;
  if (base === "package.json" || base === "cargo.toml" || base === "pyproject.toml") return 10;
  if (path.extname(base) === ".md") return 20 + file.split("/").length;
  return 100 + file.split("/").length * 10 + file.length;
}

function selectRepresentativeFiles(index) {
  const readable = index.filter(isReadableText);
  return [...readable]
    .sort((left, right) => stableFileScore(left) - stableFileScore(right) || left.localeCompare(right))
    .slice(0, 8);
}

function withSources(content, paths) {
  const unique = [...new Set(paths)];
  const normalized = content.trim();
  const missing = unique.filter((file) => !normalized.includes(`- \`${file}\``));
  if (/\n来源\s*(?:\n|$)/.test(normalized) && !missing.length) return normalized;
  return `${normalized}\n\n来源\n${missing.map((file) => `- \`${file}\``).join("\n")}`;
}

function asksForFileContent(request) {
  const value = String(request || "");
  if (/(?:根据|依据|基于).{0,10}(?:文件|源码|代码)(?:的)?内容.{0,10}(?:回答|解释|说明|概括|总结)/i.test(value)) return false;
  return /(?:[\w./\\-]+\.[a-z0-9]+|这个文件|该文件|文件|源码|代码)(?:的)?(?:完整)?内容(?:是什么|呢|[？?]|\s*$)|(?:显示|给我|贴出|输出|查看).{0,10}(?:文件|源码|代码|内容)|what(?:'s| is)\s+in/i.test(value);
}

function asksWhatFileIs(request) {
  return /(?:是什么|什么|哪种)(?:类型的)?文件|(?:做什么|作用|用途)|what\s+is.{0,40}(?:file|module)|what\s+does.{0,40}do/i.test(String(request || ""));
}

function markdownLanguage(pathname) {
  return ({ js: "javascript", jsx: "jsx", ts: "typescript", tsx: "tsx", py: "python", rs: "rust", sh: "bash", ps1: "powershell", md: "markdown" })[path.extname(pathname).slice(1).toLowerCase()]
    || path.extname(pathname).slice(1).toLowerCase()
    || "text";
}

function describeProjectFile(pathname, content) {
  const extension = path.extname(pathname).toLowerCase();
  const type = {
    ".py": "Python 源文件",
    ".js": "JavaScript 源文件",
    ".ts": "TypeScript 源文件",
    ".tsx": "TypeScript React 组件文件",
    ".rs": "Rust 源文件",
    ".md": "Markdown 文档",
    ".json": "JSON 数据或配置文件",
  }[extension] || `${extension.slice(1).toUpperCase() || "文本"} 文件`;
  const symbols = [...String(content || "").matchAll(/^(?:async\s+)?(?:def|function|class)\s+([A-Za-z_$][\w$]*)|^(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)/gm)]
    .map((match) => match[1] || match[2])
    .filter(Boolean)
    .slice(0, 6);
  const readmePurpose = extension === ".md" && /(?:^|\/)readme\.md$/i.test(pathname)
    ? describeReadmePurpose(content)
    : "";
  const detail = readmePurpose || (symbols.length
    ? `其中定义了 ${symbols.map((name) => `\`${name}\``).join("、")}。`
    : "当前内容中没有识别到可列出的函数或类定义。");
  return `\`${pathname}\` 是一个${type}。${detail}`;
}

function describeReadmePurpose(content) {
  const lines = String(content || "").split(/\r?\n/u).map((line) => line.trim()).filter(Boolean);
  const title = lines.find((line) => /^#{1,6}\s+/u.test(line))?.replace(/^#{1,6}\s+/u, "").trim();
  const summary = lines.find((line) => !/^#{1,6}\s+/u.test(line) && !/^[-*`]\s*/u.test(line));
  const subject = title ? `介绍 ${title}` : "介绍当前项目";
  return summary ? `作为项目说明文档，它用于${subject}；文件说明：${summary}` : `作为项目说明文档，它用于${subject}。`;
}

function repairWriteToolPath(tool, request) {
  if (tool?.name !== "write_file" || !tool.arguments || !requestsMutation(request)) return null;
  if (!/(?:python|py\s*文件|\.py\b|动态规划)/i.test(String(request || ""))) return null;
  const original = String(tool.arguments.path || "");
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
  return { ...tool, arguments: { ...tool.arguments, path: repairedPath } };
}
