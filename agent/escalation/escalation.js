import { createHash } from "node:crypto";
import { AppError } from "../shared/errors.js";

export function buildEscalationPayload(ticket, selectedFiles = []) {
  const payload = {
    version: 1,
    ticket: { id: ticket.id, title: ticket.title, request: ticket.trigger.request },
    reports: ticket.reports,
    selectedFiles,
    generatedAt: new Date().toISOString(),
    userReviewRequired: true,
  };
  return { payload, sha256: createHash("sha256").update(JSON.stringify(payload)).digest("hex") };
}

export function sendEscalation() {
  throw new AppError("connector_disabled", "DeepSeek connector is disabled; review and export the payload manually", 409);
}
