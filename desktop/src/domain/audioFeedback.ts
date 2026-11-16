import type { AppSettings, RuntimeEvent } from "./types";

export type AudioCue = "completed" | "approval" | "failure";
type SoundSettings = Pick<AppSettings, "soundEnabled" | "soundVolume">;
type CuePlayer = (cue: AudioCue, volume: number) => void | Promise<void>;

export function cueForEvent(eventType: string): AudioCue | null {
  if (eventType === "delivery.ready") return "completed";
  if (eventType === "effect.proposed") return "approval";
  if (["provider.error", "ticket.failed", "effect.recovery_required"].includes(eventType)) return "failure";
  return null;
}

export function clampSoundVolume(value: number): number {
  return Math.max(0, Math.min(100, Number.isFinite(value) ? Math.round(value) : 55));
}

export class AudioFeedbackController {
  private lastSequence = 0;
  private readonly failureSequenceByTicket = new Map<string, number>();

  constructor(private readonly player: CuePlayer = synthesizeCue) {}

  baseline(sequence: number) {
    if (Number.isSafeInteger(sequence)) this.lastSequence = Math.max(this.lastSequence, sequence);
  }

  process(events: RuntimeEvent[], settings: SoundSettings) {
    for (const event of [...events].sort((left, right) => left.sequence - right.sequence)) {
      if (event.sequence <= this.lastSequence) continue;
      this.lastSequence = event.sequence;
      const cue = cueForEvent(event.eventType);
      if (!cue || !settings.soundEnabled) continue;
      if (cue === "failure") {
        const ticket = event.ticketId ?? "global";
        const previous = this.failureSequenceByTicket.get(ticket);
        this.failureSequenceByTicket.set(ticket, event.sequence);
        if (previous !== undefined && event.sequence - previous <= 4) continue;
      }
      try {
        void Promise.resolve(this.player(cue, clampSoundVolume(settings.soundVolume))).catch(() => undefined);
      } catch {
        // Audio feedback is supplementary and must never affect a Ticket.
      }
    }
  }

  preview(cue: AudioCue, volume: number) {
    try {
      void Promise.resolve(this.player(cue, clampSoundVolume(volume))).catch(() => undefined);
    } catch {
      // Preview failure is intentionally non-blocking.
    }
  }
}

let context: AudioContext | null = null;

function audioContext(): AudioContext | null {
  if (context) return context;
  const AudioContextClass = window.AudioContext ?? (window as typeof window & { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
  if (!AudioContextClass) return null;
  context = new AudioContextClass();
  return context;
}

export async function unlockAudioFeedback() {
  const value = audioContext();
  if (value?.state === "suspended") await value.resume();
}

async function synthesizeCue(cue: AudioCue, volume: number) {
  const value = audioContext();
  if (!value) return;
  if (value.state === "suspended") await value.resume();
  if (value.state !== "running") return;
  const amplitude = Math.max(0.0001, Math.min(0.08, 0.08 * (clampSoundVolume(volume) / 100)));
  const patterns: Record<AudioCue, Array<{ frequency: number; offset: number; duration: number; type: OscillatorType }>> = {
    completed: [
      { frequency: 523.25, offset: 0, duration: 0.15, type: "sine" },
      { frequency: 659.25, offset: 0.105, duration: 0.16, type: "sine" },
    ],
    approval: [{ frequency: 587.33, offset: 0, duration: 0.18, type: "sine" }],
    failure: [
      { frequency: 220, offset: 0, duration: 0.17, type: "triangle" },
      { frequency: 174.61, offset: 0.13, duration: 0.18, type: "triangle" },
    ],
  };
  const now = value.currentTime + 0.008;
  for (const note of patterns[cue]) {
    const oscillator = value.createOscillator();
    const gain = value.createGain();
    const start = now + note.offset;
    const end = start + note.duration;
    oscillator.type = note.type;
    oscillator.frequency.setValueAtTime(note.frequency, start);
    gain.gain.setValueAtTime(0.0001, start);
    gain.gain.exponentialRampToValueAtTime(amplitude, start + 0.018);
    gain.gain.exponentialRampToValueAtTime(0.0001, end);
    oscillator.connect(gain);
    gain.connect(value.destination);
    oscillator.addEventListener("ended", () => { oscillator.disconnect(); gain.disconnect(); }, { once: true });
    oscillator.start(start);
    oscillator.stop(end + 0.01);
  }
}

export const audioFeedback = new AudioFeedbackController();

export function previewAudioCue(cue: AudioCue, volume: number) {
  audioFeedback.preview(cue, volume);
}
