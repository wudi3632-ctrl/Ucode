import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export function createFixture(caseDefinition, baseDirectory = os.tmpdir()) {
  const root = fs.mkdtempSync(path.join(baseDirectory, `ucode-harness-${caseDefinition.id}-`));
  for (const [relativePath, content] of Object.entries(caseDefinition.files)) {
    const target = path.join(root, ...relativePath.split("/"));
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, content, "utf8");
  }
  return root;
}

export function removeFixture(root) {
  fs.rmSync(root, { recursive: true, force: true });
}

export function indexFixture(root) {
  const output = [];
  const visit = (directory) => {
    for (const name of fs.readdirSync(directory)) {
      const target = path.join(directory, name);
      const stat = fs.statSync(target);
      if (stat.isDirectory()) visit(target);
      else output.push({ path: path.relative(root, target).replaceAll("\\", "/"), sizeBytes: stat.size, contentDigest: null });
    }
  };
  visit(root);
  return output.sort((left, right) => left.path.localeCompare(right.path));
}

export const CONTEXT_CHAR_BUDGET = 1_800;

export function snapshotFixture(root, limit = CONTEXT_CHAR_BUDGET) {
  let remaining = limit;
  const blocks = [];
  for (const file of indexFixture(root)) {
    const heading = `\n--- ${file.path} ---\n`;
    if (heading.length >= remaining) break;
    const content = fs.readFileSync(path.join(root, ...file.path.split("/")), "utf8").slice(0, remaining - heading.length);
    blocks.push(`${heading}${content}`);
    remaining -= heading.length + content.length;
  }
  return blocks.join("");
}
