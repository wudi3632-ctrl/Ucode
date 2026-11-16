# UCode Trusted MVP and Public Release Design

**Date:** 2026-11-16  
**Status:** Approved by the product owner through the recommended-path mandate  
**Target:** First credible public GitHub release

## 1. Product boundary

UCode is a portable, Windows-first, local-first code task executor. Its value is not maximum autonomy. Its value is a small, inspectable loop that helps a user understand and change a project while retaining control:

1. capture a concrete objective;
2. gather bounded evidence from the selected workspace;
3. propose a change;
4. require approval before writes;
5. execute and verify;
6. produce a concise, reversible receipt.

The product must remain useful from a USB drive, work without cloud services when a compatible local model is installed, and stop safely when evidence is weak. DeepSeek Super Agent is an architectural reference for task lifecycle and policy separation, not a claim that UCode embeds or reproduces the full framework.

## 2. Release strategies considered

### A. Trusted MVP plus public source (selected)

Fix the task loop and user-facing trust failures first, publish a source-only repository, keep model packs and portable runtimes outside Git, and publish honest capability evidence. This gives the best balance of usefulness, reproducibility, repository hygiene, and licensing safety.

### B. Publish the current research tree

Fast, but it would expose generated artifacts, unclear ownership, oversized binaries, and tests that overstate real capability. Rejected.

### C. Delay publication until full autonomous development is possible

Would substantially expand scope and conflict with UCode's focused, portable product position. Rejected.

## 3. Trustworthy task loop

### 3.1 Structured task context

Every task carries a durable context independent of chat prose:

- objective;
- target files or target area;
- constraints;
- verification command or acceptance condition;
- confirmed facts with evidence references;
- unresolved questions;
- bounded exploration counters;
- approval and rollback state.

Follow-up messages update this structure rather than replacing it. A user correction wins over inferred state. Context is persisted with the conversation and restored after restart.

### 3.2 Intent and readiness

Messages are classified into chat/explanation, project question, or executable task. Chat is never forced into a tool call. Project questions can read/search within policy but cannot write. Executable tasks advance only when objective and validation are sufficiently concrete. The composer exposes optional goal, files, constraints, and verification hints without becoming a mandatory form.

If essential information is missing, UCode asks one precise clarification. It must not fabricate file names or generate template answers unrelated to the prompt.

### 3.3 Bounded diagnosis

An autonomous diagnostic pass is allowed only within these limits:

- at most 10 files read;
- at most 2 search rounds;
- at most 1 test run before proposing a change;
- at most 1 write batch after approval;
- stop after 2 consecutive steps produce no new evidence;
- never write without explicit approval.

The limit is visible in the activity view. Reaching a limit produces a useful stop receipt, not an endless thinking state.

### 3.4 Approval and execution

Read-only operations may run within the selected workspace. Writes, creates, deletes, and destructive commands require a preview and approval. The executor validates canonical paths, rejects traversal and workspace escape, applies a transaction, records before/after hashes, verifies, and can roll back the batch.

## 4. Desktop experience

### 4.1 Information architecture

- Left rail: conversations and compact task state.
- Center: conversation, activity, approval card, and composer.
- Right inspector: files, diff, terminal, Git, agents, and evidence; resizable and collapsible.
- Settings: appearance, font size, sound, notifications, model provider, model packs, privacy, and diagnostics.

The composer remains visible at common window sizes. Rails and inspector have minimum/maximum widths and can be resized. All scroll surfaces share one subtle theme-aware scrollbar.

### 4.2 Feedback and streaming

The UI acknowledges submission in under 300 ms and shows a truthful activity event within one second. Only the active assistant response streams. Existing history is immutable during streaming. Repeated generic "thinking" rows are collapsed into one activity group with concrete verbs such as reading, searching, testing, or waiting for approval.

### 4.3 Evidence receipt

The default completion receipt answers only:

- what changed;
- why it changed;
- whether verification passed;
- remaining risk;
- how to undo it.

Hashes, raw events, tool payloads, and timing live in an expandable technical-details section. Raw tool JSON is never the primary conversation UI.

## 5. Model and provider design

- Portable default: a verified 1.5B-class local model pack when installed.
- Optional: 3B/7B local packs through the same manifest and checksum mechanism.
- Remote/local alternative: OpenAI-compatible endpoint.
- UCode policy, approval, path validation, and audit stay local regardless of provider.
- The model selector lists only discovered and verified models.
- Model weights are not stored in the source repository.

Model manifests include identity, source, license, expected files, sizes, and SHA-256 hashes. UCode must distinguish "provider reachable" from "model capable of completing this task."

## 6. Super Agent adaptation

The UCode task lifecycle maps the reference concepts as follows:

| Reference concept | UCode implementation |
| --- | --- |
| Trigger | User message or explicit retry |
| Evaluation | Intent, readiness, policy, and evidence assessment |
| Decision | Chat response, clarification, bounded investigation, or proposed change |
| Mandate | User-approved objective, constraints, and write authority |
| Ticket | Persisted task context and lifecycle state |
| Assignment | Model/provider and tool selection |
| Input | Workspace evidence and user attachments |
| Effect | Policy-checked read/test/write transaction |
| Report | Event log and verification result |
| Delivery | Human-readable receipt and rollback entry |

Rust/Tauri is the control plane and source of truth. The Node executor remains a compatibility execution layer until its responsibilities can be retired safely. Public documentation must call this a hybrid migration, not a complete upstream framework port.

## 7. Test and release gates

### 7.1 Automated gates

- desktop unit/component tests;
- Rust unit/integration tests;
- Node executor and policy tests;
- multi-turn context retention tests;
- chat/explanation routing tests;
- path escape and unapproved-write tests;
- transactional rollback tests;
- active-response-only streaming tests;
- light/dark and compact-window UI checks;
- model manifest and checksum tests;
- source tree secret and oversized-file scan.

### 7.2 Scenario benchmark

Maintain 30-50 distinct fixtures spanning chat, explanation, project questions, one-file edits, three-file edits, clarification, failed verification, rollback, and bounded diagnosis. Release targets:

- single-file tasks: at least 95%;
- three-file tasks: at least 90%;
- unapproved writes: 0;
- workspace escapes: 0;
- false completion claims: 0;
- successful rollback after induced verification failure: 100%.

Scores must report sample size, fixture diversity, model/provider, machine, and failure details.

### 7.3 Physical portability acceptance

Automated checks cannot certify physical behavior. A separate checklist covers a real USB drive, a second clean Windows machine, offline startup, changed drive letter, standard user privileges, no preinstalled development runtimes, Defender/WebView2 behavior, and unexpected drive removal. Unperformed items remain explicitly unverified.

## 8. Public repository

The repository contains source, selected tests, build scripts, architecture documents, capability boundaries, license notices, and sanitized evidence. It excludes model weights, portable runtime bundles, `node_modules`, Cargo `target`, `dist`, logs, databases, secrets, user workspaces, raw evidence, and temporary fixtures.

Required top-level documents:

- `README.md`;
- `LICENSE` (Apache-2.0);
- `BUILDING.md`;
- `ARCHITECTURE.md`;
- `CAPABILITY_BOUNDARY.md`;
- `SECURITY.md`;
- `CONTRIBUTING.md`;
- `THIRD_PARTY_NOTICES.md`.

GitHub Actions run source-only tests and hygiene scans. Releases may link separately licensed model/runtime artifacts with checksums; the Git repository must never contain the 2.395 GiB capsule.

## 9. Error handling

All failure states use structured codes and plain-language messages. Model, tool, policy, verification, and persistence failures are distinct. Retries are bounded and idempotent. A stopped task remains resumable and never reports success without a matching verification record.

## 10. Non-goals for this release

- unrestricted autonomous repository development;
- hidden background writes;
- claiming a self-trained foundation model;
- bundling 3B/7B weights in Git;
- claiming physical USB validation without executing it;
- claiming full persistent shell parity unless ConPTY behavior is verified end to end.
