# UCode Rescue Agent phase 1 implementation plan

## Goal

Ship and formally verify one honest, read-only Rescue vertical slice inside the existing desktop architecture. The slice must collect machine evidence, keep hypotheses separate from facts, let the configured model explain grounded results, and never claim a repair was completed.

## Work items

1. Add a `rescue` domain module to the Tauri host with versioned tool metadata, risk classes, evidence nodes, hypotheses, graph edges, and a bounded diagnosis report.
2. Implement a command runner boundary and real Windows R0 probes for system inventory, environment/PATH, network adapter, route, proxy, DNS and process/service summaries.
3. Add adaptive evidence analysis: observations update competing hypotheses; a failed or unavailable probe becomes an explicit unknown rather than a fabricated fact.
4. Add `rescue` to the desktop/Executor protocol and route Windows troubleshooting requests there without granting action authority.
5. Persist factual Rescue activity events and pass a bounded evidence graph to the model. Add a Rescue-specific prompt that requires evidence citations, distinguishes diagnosis from repair, and forbids completion claims.
6. Render Rescue activities and evidence/hypothesis status in the existing conversation/inspector surfaces with progressive disclosure.
7. Add formal tests:
   - registry/schema and graph invariants;
   - same-symptom/different-hidden-cause fixtures;
   - failed probe and output-bound behavior;
   - route/protocol/provider isolation;
   - UI event rendering;
   - existing Node, desktop and Rust regression suites;
   - typecheck, Vite and Tauri release builds;
   - packaged desktop smoke flow and independent machine-state verification.

## Explicit phase boundary

This phase does not execute repairs or elevate privileges. Its success state is `diagnosed` or `safe_stop`, never `verified repair`. R2 snapshots/rollback and the short-lived R3 UAC helper are follow-up phases after this read-only core passes the formal gates.
