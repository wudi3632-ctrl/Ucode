export type DiffRow = {
  kind: "context" | "addition" | "deletion" | "omission";
  oldLine: number | null;
  newLine: number | null;
  content: string;
};

export function buildDiffRows(before: string, after: string): DiffRow[] {
  const oldLines = before ? before.split(/\r?\n/) : [];
  const newLines = after ? after.split(/\r?\n/) : [];
  let prefix = 0;
  while (prefix < oldLines.length && prefix < newLines.length && oldLines[prefix] === newLines[prefix]) prefix += 1;
  let suffix = 0;
  while (suffix < oldLines.length - prefix && suffix < newLines.length - prefix && oldLines[oldLines.length - 1 - suffix] === newLines[newLines.length - 1 - suffix]) suffix += 1;
  const rows: DiffRow[] = [];
  const prefixStart = Math.max(0, prefix - 3);
  if (prefixStart > 0) rows.push({ kind: "omission", oldLine: null, newLine: null, content: `${prefixStart} 行未变更` });
  for (let index = prefixStart; index < prefix; index += 1) rows.push({ kind: "context", oldLine: index + 1, newLine: index + 1, content: oldLines[index] });
  for (let index = prefix; index < oldLines.length - suffix; index += 1) rows.push({ kind: "deletion", oldLine: index + 1, newLine: null, content: oldLines[index] });
  for (let index = prefix; index < newLines.length - suffix; index += 1) rows.push({ kind: "addition", oldLine: null, newLine: index + 1, content: newLines[index] });
  const suffixCount = Math.min(3, suffix);
  for (let offset = suffixCount; offset > 0; offset -= 1) {
    const oldIndex = oldLines.length - offset;
    const newIndex = newLines.length - offset;
    rows.push({ kind: "context", oldLine: oldIndex + 1, newLine: newIndex + 1, content: oldLines[oldIndex] });
  }
  if (suffix > suffixCount) rows.push({ kind: "omission", oldLine: null, newLine: null, content: `${suffix - suffixCount} 行未变更` });
  return rows.length ? rows : [{ kind: "context", oldLine: 1, newLine: 1, content: "文件内容未变化" }];
}

export function changedLineCounts(before: string, after: string) {
  const rows = buildDiffRows(before, after);
  return {
    additions: rows.filter((row) => row.kind === "addition").length,
    deletions: rows.filter((row) => row.kind === "deletion").length,
  };
}
