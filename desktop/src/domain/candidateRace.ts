export interface CandidateRaceRequest {
  targets: CandidateRaceTarget[];
  allowedPaths: string[];
  acceptance: string[];
}

export type CandidateRaceOperation = "create" | "modify" | "delete";

export interface CandidateRaceTarget {
  path: string;
  operation: CandidateRaceOperation;
}

const MUTATION = /(?:修复|修改|实现|新增|添加|删除|重构|创建|新建|fix|modify|implement|add|delete|remove|refactor|create)/i;
const EXTENSION = /\.(?:[cm]?[jt]sx?|py|rs|go|java|kt|swift|rb|php|cs|cpp|cc|cxx|h|hpp|json|ya?ml|toml|md|css|scss|html|vue|svelte|sql|sh|ps1)$/i;
const INVALID_PATH = /(?:^|\/)\.\.(?:\/|$)|^(?:[a-z]:|\\|\/)|[<>:"|?*]/i;

/**
 * A deliberately conservative, model-free gate for the optional two-candidate
 * path. Ambiguous work remains on the normal Agent route.
 */
export function candidateRaceRequestFromPrompt(prompt: string): CandidateRaceRequest | null {
  if (!MUTATION.test(prompt)) return null;
  const targets = extractTargets(prompt);
  if (targets.length < 2) return null;
  const acceptance = extractAcceptance(prompt);
  if (!acceptance.length) return null;
  const limitedTargets = targets.slice(0, 20);
  return { targets: limitedTargets, allowedPaths: limitedTargets.map((target) => target.path), acceptance };
}

function extractTargets(prompt: string): CandidateRaceTarget[] {
  const matches = [
    ...prompt.matchAll(/`([^`\r\n]{1,240})`/g),
    ...prompt.matchAll(/“([^”\r\n]{1,240})”/g),
    ...prompt.matchAll(/"([^"\r\n]{1,240})"/g),
    ...prompt.matchAll(/'([^'\r\n]{1,240})'/g),
    ...prompt.matchAll(/(?:^|[\s，、；;：:()（）])([\w@.+-]+(?:[\\/][\w@.+-]+)*\.[A-Za-z0-9]{1,10})(?=$|[\s，、；;。:：()（）])/g),
  ].sort((left, right) => (left.index ?? 0) - (right.index ?? 0));
  const unique = new Map<string, CandidateRaceTarget>();
  for (const match of matches) {
    const raw = String(match[1] ?? "").trim().replaceAll("\\", "/").replace(/^\.\//, "");
    if (!EXTENSION.test(raw) || INVALID_PATH.test(raw) || raw.includes("//")) continue;
    const key = raw.toLowerCase();
    if (unique.has(key)) continue;
    const operation = operationBefore(prompt, match.index ?? 0);
    if (!operation) return [];
    unique.set(key, { path: raw, operation });
  }
  return [...unique.values()];
}

function operationBefore(prompt: string, pathIndex: number): CandidateRaceOperation | null {
  const clauseStart = Math.max(
    prompt.lastIndexOf("；", pathIndex - 1),
    prompt.lastIndexOf(";", pathIndex - 1),
    prompt.lastIndexOf("。", pathIndex - 1),
    prompt.lastIndexOf("\n", pathIndex - 1),
  );
  const prefix = prompt.slice(clauseStart + 1, pathIndex);
  const verbs = [...prefix.matchAll(/创建|新建|新增|添加|删除|移除|修复|修改|实现|重构|create|add|delete|remove|fix|modify|implement|refactor/gi)];
  const verb = verbs.at(-1)?.[0].toLowerCase();
  if (!verb) return null;
  if (/^(?:创建|新建|新增|添加|create|add)$/.test(verb)) return "create";
  if (/^(?:删除|移除|delete|remove)$/.test(verb)) return "delete";
  return "modify";
}

function extractAcceptance(prompt: string): string[] {
  const commands: string[] = [];
  if (/(?:^|\W)(?:python\s+-m\s+)?pytest(?:\W|$)|运行.{0,8}(?:pytest|测试)|执行.{0,8}(?:pytest|测试)/i.test(prompt)) commands.push("pytest");
  if (/(?:^|\W)cargo\s+test(?:\W|$)/i.test(prompt)) commands.push("cargo test");
  if (/(?:^|\W)npm\s+test(?:\W|$)/i.test(prompt)) commands.push("npm test");
  for (const match of prompt.matchAll(/(?:^|\W)npm\s+run\s+(test|typecheck|build|check|lint)(?=$|\W)/gi)) {
    commands.push(`npm run ${match[1].toLowerCase()}`);
  }
  return [...new Set(commands)];
}
