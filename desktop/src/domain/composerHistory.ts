export interface ComposerHistoryState {
  cursor: number | null;
  draft: string;
}

export interface ComposerHistoryResult extends ComposerHistoryState {
  value: string;
}

export function buildComposerHistory(values: string[], limit = 50) {
  const history: string[] = [];
  for (const raw of values) {
    const value = raw.trim();
    if (!value || history.at(-1) === value) continue;
    history.push(value);
  }
  return history.slice(-Math.max(0, limit));
}

export function canStartComposerHistory(value: string, selectionStart: number | null, selectionEnd: number | null) {
  if (!value) return true;
  return selectionStart === 0 && selectionEnd === 0;
}

export function moveComposerHistory(
  history: string[],
  state: ComposerHistoryState,
  currentValue: string,
  direction: "older" | "newer",
): ComposerHistoryResult | null {
  if (!history.length) return null;
  if (direction === "older") {
    const cursor = state.cursor === null ? history.length - 1 : Math.max(0, state.cursor - 1);
    return { cursor, draft: state.cursor === null ? currentValue : state.draft, value: history[cursor] };
  }
  if (state.cursor === null) return null;
  if (state.cursor >= history.length - 1) return { cursor: null, draft: state.draft, value: state.draft };
  const cursor = state.cursor + 1;
  return { cursor, draft: state.draft, value: history[cursor] };
}

export function exitComposerHistory(state: ComposerHistoryState): ComposerHistoryResult | null {
  return state.cursor === null ? null : { cursor: null, draft: state.draft, value: state.draft };
}
