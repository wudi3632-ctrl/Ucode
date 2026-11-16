import type { RuntimeEvent } from "./types";

export type ObservedModelOutcome = "generating" | "completed" | "cancelled" | "failed";

export interface ObservedModelPerformance {
  outcome: ObservedModelOutcome;
  active: boolean;
  requestId: string | null;
  firstCharacterMs: number | null;
  charactersPerSecond: number | null;
  characterCount: number;
  startedAt: string;
  firstOutputAt: string | null;
  lastOutputAt: string | null;
  endedAt: string | null;
}

const MIN_RATE_INTERVAL_MS = 250;

interface Attempt {
  startedAt: string;
  startedMs: number | null;
  requestId: string | null;
  outcome: ObservedModelOutcome;
  endedAt: string | null;
  characterCount: number;
  nonEmptyDeltas: number;
  firstOutputAt: string | null;
  firstOutputMs: number | null;
  lastOutputAt: string | null;
  lastOutputMs: number | null;
  streamSequences: Set<string>;
}

export function deriveObservedModelPerformance(events: RuntimeEvent[]): ObservedModelPerformance | null {
  let attempt: Attempt | null = null;
  for (const event of [...events].sort((left, right) => left.sequence - right.sequence)) {
    if (event.eventType === "provider.started") {
      attempt = createAttempt(event);
      continue;
    }
    if (event.eventType === "provider.stream.started") {
      const requestId = stringValue(event.payload.requestId);
      if (!attempt || attempt.outcome !== "generating" || attempt.nonEmptyDeltas > 0 || (attempt.requestId && attempt.requestId !== requestId)) {
        attempt = createAttempt(event, requestId);
      } else if (requestId) {
        attempt.requestId = requestId;
      }
      continue;
    }
    if (!attempt) continue;

    if (event.eventType === "provider.delta") {
      appendDelta(attempt, event);
      continue;
    }
    if (["delivery.ready", "provider.message"].includes(event.eventType)) {
      if (attempt.outcome === "generating" || attempt.outcome === "completed") {
        attempt.outcome = "completed";
        attempt.endedAt = event.createdAt;
      }
      continue;
    }
    if (["provider.cancelled", "ticket.cancelled"].includes(event.eventType)) {
      if (attempt.outcome === "generating" && requestMatches(attempt, event)) {
        attempt.outcome = "cancelled";
        attempt.endedAt = event.createdAt;
      }
      continue;
    }
    if (["provider.error", "ticket.failed"].includes(event.eventType) && attempt.outcome === "generating") {
      attempt.outcome = "failed";
      attempt.endedAt = event.createdAt;
    }
  }
  if (!attempt) return null;

  const firstCharacterMs = duration(attempt.startedMs, attempt.firstOutputMs);
  const outputIntervalMs = duration(attempt.firstOutputMs, attempt.lastOutputMs);
  const charactersPerSecond = attempt.nonEmptyDeltas >= 2 && outputIntervalMs !== null && outputIntervalMs >= MIN_RATE_INTERVAL_MS
    ? round(attempt.characterCount / (outputIntervalMs / 1000), 1)
    : null;
  return {
    outcome: attempt.outcome,
    active: attempt.outcome === "generating",
    requestId: attempt.requestId,
    firstCharacterMs,
    charactersPerSecond,
    characterCount: attempt.characterCount,
    startedAt: attempt.startedAt,
    firstOutputAt: attempt.firstOutputAt,
    lastOutputAt: attempt.lastOutputAt,
    endedAt: attempt.endedAt,
  };
}

function createAttempt(event: RuntimeEvent, requestId = stringValue(event.payload.requestId)): Attempt {
  return {
    startedAt: event.createdAt,
    startedMs: timestamp(event.createdAt),
    requestId,
    outcome: "generating",
    endedAt: null,
    characterCount: 0,
    nonEmptyDeltas: 0,
    firstOutputAt: null,
    firstOutputMs: null,
    lastOutputAt: null,
    lastOutputMs: null,
    streamSequences: new Set(),
  };
}

function appendDelta(attempt: Attempt, event: RuntimeEvent) {
  const requestId = stringValue(event.payload.requestId);
  if (attempt.requestId && requestId && attempt.requestId !== requestId) return;
  if (!attempt.requestId && requestId) attempt.requestId = requestId;
  const streamSequence = event.payload.streamSequence;
  if (streamSequence !== undefined && streamSequence !== null) {
    const key = `${attempt.requestId ?? "legacy"}:${String(streamSequence)}`;
    if (attempt.streamSequences.has(key)) return;
    attempt.streamSequences.add(key);
  }
  const delta = typeof event.payload.delta === "string" ? event.payload.delta : "";
  if (!delta) return;
  const at = timestamp(event.createdAt);
  attempt.characterCount += Array.from(delta).length;
  attempt.nonEmptyDeltas += 1;
  if (attempt.firstOutputAt === null) {
    attempt.firstOutputAt = event.createdAt;
    attempt.firstOutputMs = at;
  }
  attempt.lastOutputAt = event.createdAt;
  attempt.lastOutputMs = at;
}

function requestMatches(attempt: Attempt, event: RuntimeEvent) {
  const requestId = stringValue(event.payload.requestId);
  return !attempt.requestId || !requestId || attempt.requestId === requestId;
}

function stringValue(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const normalized = value.trim();
  return normalized || null;
}

function timestamp(value: string): number | null {
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function duration(start: number | null, end: number | null): number | null {
  if (start === null || end === null || end < start) return null;
  return end - start;
}

function round(value: number, digits: number) {
  const factor = 10 ** digits;
  return Math.round(value * factor) / factor;
}
