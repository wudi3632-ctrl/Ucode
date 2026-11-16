import { describe, expect, it, vi } from "vitest";
import { CompletionNotificationController, COMPLETION_NOTIFICATION_BODY, COMPLETION_NOTIFICATION_TITLE } from "./completionNotifications";
import type { RuntimeEvent } from "./types";

const runtimeEvent = (sequence: number, eventType = "delivery.ready"): RuntimeEvent => ({
  sequence,
  ticketId: "ticket-1",
  eventType,
  payload: {},
  createdAt: new Date(0).toISOString(),
});

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("background completion notifications", () => {
  it("uses fixed privacy-safe copy", () => {
    expect(COMPLETION_NOTIFICATION_TITLE).toBe("UCode · 任务完成");
    expect(COMPLETION_NOTIFICATION_BODY).toBe("任务已完成，点击返回 UCode");
    expect(`${COMPLETION_NOTIFICATION_TITLE}${COMPLETION_NOTIFICATION_BODY}`).not.toMatch(/project|path|prompt|ticket/i);
  });

  it("baselines history, stays silent in foreground, and emits once in background", async () => {
    const activity = vi.fn()
      .mockResolvedValueOnce({ focused: true, visible: true, minimized: false })
      .mockResolvedValueOnce({ focused: false, visible: true, minimized: false });
    const send = vi.fn().mockResolvedValue(undefined);
    const controller = new CompletionNotificationController(activity, send);
    controller.baseline(5);
    controller.process([runtimeEvent(4), runtimeEvent(6)], { completionNotificationsEnabled: true });
    await settle();
    expect(send).not.toHaveBeenCalled();
    controller.process([runtimeEvent(6), runtimeEvent(7)], { completionNotificationsEnabled: true });
    await settle();
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("advances while disabled and does not replay after enabling", async () => {
    const send = vi.fn().mockResolvedValue(undefined);
    const controller = new CompletionNotificationController(
      vi.fn().mockResolvedValue({ focused: false, visible: false, minimized: false }),
      send,
    );
    controller.process([runtimeEvent(1)], { completionNotificationsEnabled: false });
    controller.process([runtimeEvent(1)], { completionNotificationsEnabled: true });
    await settle();
    expect(send).not.toHaveBeenCalled();
  });

  it("ignores non-completion events and isolates activity or sender failures", async () => {
    const sender = vi.fn().mockRejectedValue(new Error("notifications unavailable"));
    const controller = new CompletionNotificationController(
      vi.fn().mockResolvedValue({ focused: false, visible: false, minimized: true }),
      sender,
    );
    controller.process([runtimeEvent(1, "provider.delta"), runtimeEvent(2)], { completionNotificationsEnabled: true });
    await settle();
    expect(sender).toHaveBeenCalledTimes(1);
    const broken = new CompletionNotificationController(vi.fn().mockRejectedValue(new Error("window gone")), vi.fn());
    expect(() => broken.process([runtimeEvent(3)], { completionNotificationsEnabled: true })).not.toThrow();
    await settle();
  });
});

