export const UI_FONT_MIN = 14;
export const UI_FONT_MAX = 32;
export const UI_FONT_DEFAULT = 18;
export const CODE_FONT_MIN = 12;
export const CODE_FONT_MAX = 28;
export const CODE_FONT_DEFAULT = 14;

export function clampFontSize(value: number, min: number, max: number, fallback: number): number {
  if (!Number.isFinite(value)) return fallback;
  return Math.max(min, Math.min(max, Math.round(value)));
}

export function uiFontSize(value: number): number {
  return clampFontSize(value, UI_FONT_MIN, UI_FONT_MAX, UI_FONT_DEFAULT);
}

export function codeFontSize(value: number): number {
  return clampFontSize(value, CODE_FONT_MIN, CODE_FONT_MAX, CODE_FONT_DEFAULT);
}

export function uiFontScale(value: number): number {
  return uiFontSize(value) / 14;
}

export function codeFontScale(value: number): number {
  return codeFontSize(value) / 12;
}
