import type { AppSettings, RuntimeEvent } from "./types";
import type { MainWindowActivity } from "../bridge/desktop";

type NotificationSettings = Pick<AppSettings, "completionNotificationsEnabled">;
type ActivityReader = () => Promise<MainWindowActivity>;
type NotificationSender = () => Promise<void>;

export const COMPLETION_NOTIFICATION_TITLE = "UCode · 任务完成";
export const COMPLETION_NOTIFICATION_BODY = "任务已完成，点击返回 UCode";

export class CompletionNotificationController {
  private lastSequence = 0;

  constructor(
    private readonly readActivity: ActivityReader,
    private readonly send: NotificationSender,
  ) {}

  baseline(sequence: number) {
    this.lastSequence = Math.max(this.lastSequence, sequence);
  }

  process(events: RuntimeEvent[], settings: NotificationSettings) {
    for (const event of [...events].sort((left, right) => left.sequence - right.sequence)) {
      if (event.sequence <= this.lastSequence) continue;
      this.lastSequence = event.sequence;
      if (event.eventType !== "delivery.ready" || !settings.completionNotificationsEnabled) continue;
      void this.deliverWhenBackgrounded();
    }
  }

  private async deliverWhenBackgrounded() {
    try {
      const activity = await this.readActivity();
      if (activity.focused && activity.visible && !activity.minimized) return;
      await this.send();
    } catch {
      // Notification availability must never change ticket state or surface a blocking error.
    }
  }
}

