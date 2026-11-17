function percent(value) {
  return value == null ? "n/a" : `${(value * 100).toFixed(1)}%`;
}

function milliseconds(value) {
  return `${Math.round(value)} ms`;
}

export function renderReport({ metadata, results, summary }) {
  const lines = [
    "# UCode Same-model Harness Comparison",
    "",
    `- Run mode: **${metadata.mode}**`,
    `- Model health: \`${JSON.stringify(metadata.health)}\``,
    `- Git commit: \`${metadata.gitCommit}\``,
    `- Cases: ${metadata.caseIds.join(", ")}`,
    "",
    "## Summary",
    "",
    "| Group | Success | False completion | Escape | Avg latency | Model calls | Tool calls | Agent activations |",
    "|---|---:|---:|---:|---:|---:|---:|---:|",
  ];
  for (const [group, value] of Object.entries(summary.groups)) {
    lines.push(`| ${group} | ${value.successes}/${value.cases} (${percent(value.successRate)}) | ${value.falseCompletions} | ${value.workspaceEscapes} | ${milliseconds(value.averageElapsedMs)} | ${value.modelCalls} | ${value.toolCalls} | ${value.agentActivations} |`);
  }
  lines.push("", "## Team gate", "");
  if (!summary.decision) lines.push("The selected groups do not include both UCode Single and UCode Team.");
  else {
    lines.push(
      `- Cross-file gain: ${percent(summary.decision.crossFileGain)}`,
      `- Latency ratio: ${summary.decision.latencyRatio == null ? "n/a" : `${summary.decision.latencyRatio.toFixed(2)}×`}`,
      `- False completion non-increasing: ${summary.decision.falseCompletionNonIncreasing ? "yes" : "no"}`,
      `- No simple-task Agent activation: ${summary.decision.noSimpleTaskActivation ? "yes" : "no"}`,
      `- Meets automatic-Team threshold: **${summary.decision.meetsTeamThreshold ? "yes" : "no"}**`,
    );
  }
  lines.push(
    "",
    "## Per-case evidence",
    "",
    "| Case | Group | Outcome | Elapsed | Calls | Tools | Agents | Failure notes |",
    "|---|---|---|---:|---:|---:|---:|---|",
  );
  for (const result of results) {
    lines.push(`| ${result.caseId} | ${result.group} | ${result.score.outcome}${result.score.success ? " ✓" : ""} | ${milliseconds(result.elapsedMs)} | ${result.metrics.modelCalls} | ${result.toolCalls} | ${result.activatedAgents} | ${result.score.failures.join("; ").replaceAll("|", "\\|") || "-"} |`);
  }
  lines.push(
    "",
    metadata.mode === "scripted-ci"
      ? "> Scripted mode validates benchmark mechanics only. It is not model-quality evidence. Only live runs may be used for product claims."
      : "> This is a live local-model run. Results include failures and latency and may be used as bounded evidence only for the listed model, fixtures and Git commit.",
    "",
  );
  return lines.join("\n");
}
