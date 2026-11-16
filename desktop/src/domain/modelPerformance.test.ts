import { describe, expect, it } from "vitest";
import type { RuntimeEvent } from "./types";
import { deriveObservedModelPerformance } from "./modelPerformance";

function event(sequence: number, eventType: string, at: string, payload: Record<string, unknown> = {}): RuntimeEvent {
  return { sequence, eventType, payload, ticketId: "ticket-1", createdAt: at };
}

describe("observed model performance", () => {
  it("derives one completed end-to-end streaming sample", () => {
    const value = deriveObservedModelPerformance([
      event(1, "provider.started", "2026-11-11T00:00:00.000Z"),
      event(2, "provider.stream.started", "2026-11-11T00:00:00.100Z", { requestId: "request-1" }),
      event(3, "provider.delta", "2026-11-11T00:00:01.200Z", { requestId: "request-1", streamSequence: 1, delta: "你好" }),
      event(4, "provider.delta", "2026-11-11T00:00:01.700Z", { requestId: "request-1", streamSequence: 2, delta: "，世" }),
      event(5, "provider.delta", "2026-11-11T00:00:02.200Z", { requestId: "request-1", streamSequence: 3, delta: "界" }),
      event(6, "delivery.ready", "2026-11-11T00:00:02.400Z", { content: "你好，世界" }),
    ]);

    expect(value).toMatchObject({
      outcome: "completed",
      active: false,
      firstCharacterMs: 1200,
      characterCount: 5,
      charactersPerSecond: 5,
      requestId: "request-1",
    });
  });

  it("does not invent a rate for one buffered output chunk", () => {
    const value = deriveObservedModelPerformance([
      event(1, "provider.started", "2026-11-11T00:00:00.000Z"),
      event(2, "provider.delta", "2026-11-11T00:00:00.800Z", { delta: "一次返回", streamSequence: 1 }),
      event(3, "provider.message", "2026-11-11T00:00:00.900Z", { content: "一次返回" }),
    ]);
    expect(value).toMatchObject({ outcome: "completed", firstCharacterMs: 800, characterCount: 4, charactersPerSecond: null });
  });

  it("retains partial measurements for cancellation and failure", () => {
    const cancelled = deriveObservedModelPerformance([
      event(1, "provider.started", "2026-11-11T00:00:00.000Z"),
      event(2, "provider.stream.started", "2026-11-11T00:00:00.050Z", { requestId: "cancelled" }),
      event(3, "provider.delta", "2026-11-11T00:00:01.000Z", { requestId: "cancelled", streamSequence: 1, delta: "部分" }),
      event(4, "provider.delta", "2026-11-11T00:00:01.500Z", { requestId: "cancelled", streamSequence: 2, delta: "回答" }),
      event(5, "provider.cancelled", "2026-11-11T00:00:01.700Z", { requestId: "cancelled" }),
    ]);
    expect(cancelled).toMatchObject({ outcome: "cancelled", active: false, firstCharacterMs: 1000, characterCount: 4, charactersPerSecond: 8 });

    const failed = deriveObservedModelPerformance([
      event(1, "provider.started", "2026-11-11T00:00:00.000Z"),
      event(2, "provider.error", "2026-11-11T00:00:00.500Z", { error: "offline" }),
    ]);
    expect(failed).toMatchObject({ outcome: "failed", active: false, firstCharacterMs: null, characterCount: 0, charactersPerSecond: null });
  });

  it("keeps only the latest attempt and ignores replayed or foreign deltas", () => {
    const value = deriveObservedModelPerformance([
      event(1, "provider.started", "2026-11-11T00:00:00.000Z"),
      event(2, "provider.delta", "2026-11-11T00:00:00.300Z", { delta: "旧回答", streamSequence: 1 }),
      event(3, "delivery.ready", "2026-11-11T00:00:00.400Z", { content: "旧回答" }),
      event(4, "user.message", "2026-11-11T00:00:01.000Z", { content: "继续" }),
      event(5, "provider.started", "2026-11-11T00:00:02.000Z"),
      event(6, "provider.stream.started", "2026-11-11T00:00:02.050Z", { requestId: "new" }),
      event(7, "provider.delta", "2026-11-11T00:00:02.500Z", { requestId: "foreign", streamSequence: 1, delta: "不要计数" }),
      event(8, "provider.delta", "2026-11-11T00:00:03.000Z", { requestId: "new", streamSequence: 1, delta: "新" }),
      event(9, "provider.delta", "2026-11-11T00:00:03.100Z", { requestId: "new", streamSequence: 1, delta: "重复" }),
      event(10, "provider.delta", "2026-11-11T00:00:03.500Z", { requestId: "new", streamSequence: 2, delta: "回答" }),
    ]);
    expect(value).toMatchObject({ outcome: "generating", active: true, requestId: "new", firstCharacterMs: 1000, characterCount: 3, charactersPerSecond: 6 });
  });

  it("degrades invalid timestamps to unavailable fields", () => {
    const value = deriveObservedModelPerformance([
      event(1, "provider.started", "not-a-time"),
      event(2, "provider.delta", "2026-11-11T00:00:00.000Z", { delta: "内容", streamSequence: 1 }),
      event(3, "provider.delta", "2026-11-11T00:00:00.500Z", { delta: "继续", streamSequence: 2 }),
      event(4, "delivery.ready", "2026-11-11T00:00:00.600Z", { content: "内容继续" }),
    ]);
    expect(value).toMatchObject({ outcome: "completed", firstCharacterMs: null, characterCount: 4, charactersPerSecond: 8 });
    expect(deriveObservedModelPerformance([])).toBeNull();
  });
});
