import { describe, expect, it } from "vitest";
import { codeFontScale, codeFontSize, uiFontScale, uiFontSize } from "./typography";

describe("typography presets", () => {
  it("uses direct pixel sizes with bounded proportional CSS variables", () => {
    expect(uiFontSize(8)).toBe(14);
    expect(uiFontSize(18.4)).toBe(18);
    expect(uiFontSize(99)).toBe(32);
    expect(codeFontSize(4)).toBe(12);
    expect(codeFontSize(99)).toBe(28);
    expect(uiFontScale(18)).toBeCloseTo(18 / 14);
    expect(codeFontScale(14)).toBeCloseTo(14 / 12);
  });
});
