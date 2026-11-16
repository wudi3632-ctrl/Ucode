import { describe, expect, it, vi } from "vitest";
import { AudioFeedbackController, clampSoundVolume, cueForEvent } from "./audioFeedback";
import type { RuntimeEvent } from "./types";

function event(sequence: number, eventType: string, ticketId = "ticket-a"): RuntimeEvent {
  return { sequence, ticketId, eventType, payload: {}, createdAt: "2026-11-11T00:00:00.000Z" };
}

describe("audio feedback", () => {
  it("maps only the three approved semantic states", () => {
    expect(cueForEvent("delivery.ready")).toBe("completed");
    expect(cueForEvent("effect.proposed")).toBe("approval");
    expect(cueForEvent("effect.recovery_required")).toBe("failure");
    expect(cueForEvent("provider.delta")).toBeNull();
    expect(cueForEvent("ticket.cancelled")).toBeNull();
  });

  it("clamps malformed and excessive volume", () => {
    expect(clampSoundVolume(-20)).toBe(0);
    expect(clampSoundVolume(48.6)).toBe(49);
    expect(clampSoundVolume(180)).toBe(100);
    expect(clampSoundVolume(Number.NaN)).toBe(55);
  });

  it("does not replay the bootstrap baseline or silent events", () => {
    const play = vi.fn();
    const controller = new AudioFeedbackController(play);
    controller.baseline(10);
    controller.process([event(4, "delivery.ready"), event(11, "provider.delta"), event(12, "ticket.cancelled")], { soundEnabled: true, soundVolume: 55 });
    expect(play).not.toHaveBeenCalled();
  });

  it("plays new cues once and suppresses adjacent failure duplicates", () => {
    const play = vi.fn();
    const controller = new AudioFeedbackController(play);
    controller.process([
      event(1, "delivery.ready"),
      event(2, "effect.proposed"),
      event(3, "provider.error"),
      event(4, "ticket.failed"),
      event(5, "delivery.ready"),
    ], { soundEnabled: true, soundVolume: 61 });
    expect(play.mock.calls).toEqual([["completed", 61], ["approval", 61], ["failure", 61], ["completed", 61]]);
    controller.process([event(5, "delivery.ready")], { soundEnabled: true, soundVolume: 61 });
    expect(play).toHaveBeenCalledTimes(4);
  });

  it("advances while muted and isolates synchronous or asynchronous audio failures", async () => {
    const play = vi.fn().mockImplementationOnce(() => { throw new Error("blocked"); }).mockRejectedValueOnce(new Error("suspended"));
    const controller = new AudioFeedbackController(play);
    controller.process([event(1, "delivery.ready")], { soundEnabled: false, soundVolume: 55 });
    controller.process([event(1, "delivery.ready"), event(2, "effect.proposed"), event(3, "delivery.ready")], { soundEnabled: true, soundVolume: 55 });
    await Promise.resolve();
    expect(play).toHaveBeenCalledTimes(2);
  });
});
