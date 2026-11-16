# Architecture

## Product loop

```text
User message
  -> deterministic intent/readiness evaluation
  -> persisted task context and mandate
  -> bounded evidence collection
  -> model decision or proposed effect
  -> local policy and explicit approval
  -> transactional execution
  -> verification, receipt, and rollback
```

## Components

### Tauri/Rust control plane

Owns portable paths, SQLite state, task lifecycle, request idempotency, provider configuration, model-pack verification, terminal sessions, Git/worktrees, desktop notifications, approval resolution, evidence receipts, and event delivery. Security decisions are made here rather than delegated to the model.

### React desktop

Renders conversations, factual activity, task context, approval cards, receipts, files, diff, terminal, Git, Agents, settings, onboarding, tray-aware status, and recovery states. Only the current assistant answer streams; previously committed history is immutable.

### Node executor compatibility layer

Provides bounded chat/project/action execution and tool adapters. It has no independent desktop authority. Rust supplies workspace inventory, mode, assignment, and mandate through the versioned executor protocol. This layer exists for compatibility during migration and is not the source of truth for policy.

### Model providers

A selected local GGUF pack runs through the portable inference runtime, or the user may configure an OpenAI-compatible endpoint. Provider reachability is not treated as proof of task competence. Model identity is displayed only after discovery and verification.

## Super Agent adaptation

UCode adopts lifecycle ideas from DeepSeek's `dsh-superagent-prototype`:

- Trigger: a user request or retry;
- Evaluation: intent, readiness, and policy assessment;
- Decision: chat, read, clarify, accept, or stop;
- Mandate: immutable authority for a ticket;
- Ticket: durable task state;
- Assignment/Input: bounded work and messages;
- Effect: proposed, approved, applied, failed, or rolled-back change;
- Report/Delivery: factual progress and user-facing result.

The implementation is intentionally hybrid. Rust contains the lifecycle control plane while Node still implements compatibility execution. UCode does not claim to embed the complete upstream framework.

## Safety invariants

- canonical project roots and indexed relative paths;
- no action request without matching assignment and mandate;
- no write before effect preview and explicit approval;
- digest revalidation immediately before applying a batch;
- all-or-nothing file transaction and retained recovery evidence;
- bounded diagnosis: at most 10 reads, 2 searches, 1 pre-change test, 1 write batch, and stop after 2 repeated evidence steps;
- idempotent client requests and monotonic event streams;
- API keys are kept in Windows Credential Manager, never settings or logs.

