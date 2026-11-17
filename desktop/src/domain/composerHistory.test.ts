import { describe, expect, it } from "vitest";
import { buildComposerHistory, canStartComposerHistory, exitComposerHistory, moveComposerHistory } from "./composerHistory";

describe("composer history", () => {
  it("keeps the newest 50 non-empty messages and collapses consecutive duplicates", () => {
    const values = ["", "same", "same", ...Array.from({ length: 55 }, (_, index) => `message ${index}`)];
    const history = buildComposerHistory(values);
    expect(history).toHaveLength(50);
    expect(history[0]).toBe("message 5");
    expect(history.at(-1)).toBe("message 54");
  });

  it("moves older and newer before restoring the original draft", () => {
    const history = ["first", "second", "third"];
    const newest = moveComposerHistory(history, { cursor: null, draft: "" }, "draft", "older")!;
    expect(newest).toEqual({ cursor: 2, draft: "draft", value: "third" });
    const older = moveComposerHistory(history, newest, newest.value, "older")!;
    expect(older.value).toBe("second");
    const newer = moveComposerHistory(history, older, older.value, "newer")!;
    expect(newer.value).toBe("third");
    expect(moveComposerHistory(history, newer, newer.value, "newer")).toEqual({ cursor: null, draft: "draft", value: "draft" });
  });

  it("starts only from an empty draft or the absolute caret start", () => {
    expect(canStartComposerHistory("", 0, 0)).toBe(true);
    expect(canStartComposerHistory("draft", 0, 0)).toBe(true);
    expect(canStartComposerHistory("draft", 2, 2)).toBe(false);
    expect(canStartComposerHistory("draft", 0, 3)).toBe(false);
  });

  it("exits to the preserved draft only while browsing", () => {
    expect(exitComposerHistory({ cursor: 1, draft: "work in progress" })).toEqual({ cursor: null, draft: "work in progress", value: "work in progress" });
    expect(exitComposerHistory({ cursor: null, draft: "work in progress" })).toBeNull();
  });
});
