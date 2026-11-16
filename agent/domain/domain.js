import { randomUUID } from "node:crypto";
import { AppError } from "../shared/errors.js";

export const TicketState = Object.freeze({
  OPEN: "open",
  WAITING: "waiting",
  RUNNING: "running",
  STOPPED: "stopped",
  ENDED: "ended",
  FAILED: "failed",
  ROLLED_BACK: "rolled_back",
});

export const ReportKind = Object.freeze({
  PROGRESS: "progress",
  WAITING: "waiting",
  READY: "ready",
  BLOCKED: "blocked",
  FAILED: "failed",
  DROPPED: "dropped",
});

export function initialMessages(request) {
  return [
    { role: "system", content: "你是 UCode，一个可对话的本地编码助手。普通问答、澄清和讨论请直接用简洁自然的中文回答；只有确实需要读取、搜索、测试或修改项目时才调用工具。所有项目操作必须通过工具，写文件必须等待用户批准。使用工具时只输出合法的工具调用；不使用工具时输出正常回答。记住当前消息历史并延续上下文。" },
    { role: "user", content: request },
  ];
}

export function createTicket(projectRoot, request) {
  const now = new Date().toISOString();
  return {
    id: `ticket-${randomUUID()}`,
    source: "local-project",
    trigger: { id: randomUUID(), actor: "local-user", request, at: now },
    mandate: { strength: "explicit", scope: projectRoot, policyVersion: 1 },
    projectRoot,
    title: request.slice(0, 100),
    state: TicketState.OPEN,
    assignments: [],
    effects: [],
    reports: [],
    execution: { messages: initialMessages(request), turns: 0, pendingEffectId: null, testsPassed: false },
    createdAt: now,
    updatedAt: now,
  };
}

export function assign(ticket) {
  if (![TicketState.OPEN, TicketState.WAITING].includes(ticket.state)) {
    throw new AppError("invalid_ticket_state", `Cannot assign a ${ticket.state} ticket`, 409);
  }
  const assignment = { id: `assignment-${randomUUID()}`, state: "running", createdAt: new Date().toISOString() };
  ticket.assignments.push(assignment);
  ticket.state = TicketState.RUNNING;
  ticket.updatedAt = assignment.createdAt;
  return assignment;
}

export function report(ticket, kind, detail) {
  const item = { id: randomUUID(), kind, detail, at: new Date().toISOString() };
  ticket.reports.push(item);
  ticket.updatedAt = item.at;
  if (kind === ReportKind.WAITING) ticket.state = TicketState.WAITING;
  if (kind === ReportKind.READY) ticket.state = TicketState.ENDED;
  if (kind === ReportKind.DROPPED) ticket.state = TicketState.STOPPED;
  if (kind === ReportKind.FAILED) ticket.state = TicketState.FAILED;
  return item;
}
