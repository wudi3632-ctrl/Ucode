# UCode Rescue Agent — approved product and architecture design

Date: 2026-11-18  
Status: Approved for implementation  
Target: Windows 10/11 x64, USB-portable, local-first

## 1. Product promise

UCode Rescue is a portable Windows diagnosis and repair workbench. A user plugs in the USB drive, describes a symptom in ordinary language, and UCode gathers evidence, narrows the cause, proposes the smallest safe repair, asks for approval when required, applies it locally, and verifies the original symptom again.

The differentiator is not a larger chat model or a larger tool list. It is a trustworthy local harness that can make small or remote models useful without surrendering execution authority:

> Diagnose, repair, and prove the result on an unfamiliar Windows PC with minimal user operation.

Project and coding repair remain supported, but are one Rescue domain rather than the whole product.

## 2. Non-goals

- UCode must not claim to solve every Windows failure automatically.
- It must not map symptoms to canned repair scripts or claim success because a command exited with zero.
- It must not let a model directly own permissions, elevation, execution, backups, or verification.
- It must not silently upload machine data or require a network connection for the core diagnosis path.
- It must not treat a safe stop, an approval request, or a plausible explanation as task completion.
- The first slice does not include arbitrary elevated mutation. Elevation follows only after the read-only diagnosis core is proven.

## 3. Operating modes

1. **Fully offline** — bundled model and local tools only. This is always available when the capsule is healthy.
2. **Ask before cloud enhancement** — default. UCode previews the exact data categories and approximate size before sending any content to a network provider.
3. **User-selected provider** — bundled model or OpenAI-compatible endpoint. Native Anthropic and Gemini adapters are later extensions behind the same event contract.

In every mode the provider only reasons and proposes. Local UCode owns policy, execution, elevation, snapshots, verification, rollback, and the audit receipt.

## 4. Dynamic diagnosis loop

Diagnosis is hypothesis-driven, not keyword-driven:

1. Convert the reported symptom into a testable objective and observed facts.
2. Create several falsifiable hypotheses, including an `unknown` branch.
3. Select the next probe by information value, safety, cost, and relevance.
4. Execute one or a small batch of bounded read-only tools.
5. Add immutable observations to the evidence graph.
6. Update confidence and mark hypotheses as supported, contradicted, excluded, or unresolved.
7. Stop when one cause is sufficiently supported, the probe budget is exhausted, or further work requires user/elevated authority.
8. Propose the minimum mutation with prerequisites, expected effect, verification, and rollback.
9. After approval, snapshot affected state, execute, and rerun the original symptom check.

The planner may use a model, deterministic heuristics, or both. It may not fabricate observations. Every conclusion shown as a fact must reference evidence produced by a tool or explicitly supplied by the user.

## 5. Evidence graph

The graph is persisted with the task and is append-only for observations.

### Nodes

- `symptom`: what the user reports and the desired outcome.
- `fact`: a tool or user observation with provenance and timestamp.
- `hypothesis`: a falsifiable possible cause with confidence and state.
- `probe`: a planned tool call and why it is useful.
- `action`: a proposed or executed mutation.
- `verification`: a retest of the original symptom or action invariant.
- `unknown`: an explicit gap that prevents a stronger conclusion.

### Edges

- `supports`, `contradicts`, `tests`, `derived_from`, `requires`, `verifies`, `invalidates`.

Each evidence item records tool id/version, normalized arguments, exit status, bounded stdout/stderr, structured data, sensitivity labels, content digest, and collection time. UI summaries link back to this source record.

## 6. Tool Registry

Tools are fixed, versioned, schema-validated capabilities. A model selects tools but cannot invent a tool or bypass its validator.

Every definition contains:

- stable id and version;
- domain and description;
- JSON input/output schemas;
- risk class and required authority;
- supported platforms and prerequisites;
- timeout, output, and concurrency bounds;
- sensitivity classification;
- executor implementation;
- optional snapshot, verification, and rollback contracts.

Initial read-only registry:

- `system.inventory`
- `system.process.list`
- `system.service.inspect`
- `system.eventlog.query`
- `system.environment.inspect`
- `network.adapter.inspect`
- `network.route.inspect`
- `network.proxy.inspect`
- `network.dns.resolve`
- `network.tcp.probe`
- `filesystem.read_text`
- `filesystem.hash`
- `filesystem.permissions.inspect`
- `shell.powershell.readonly`

Verified operation modules may encode safe mechanics and rollback, but they may not predeclare a diagnosis.

## 7. Risk, permission, and elevation

| Class | Meaning | Default behavior |
|---|---|---|
| R0 | Pure local observation | Run automatically within bounds |
| R1 | Sensitive observation or optional network probe | Show purpose/data boundary; apply settings and consent |
| R2 | Reversible mutation | Show plan, snapshot, require approval |
| R3 | Privileged or broadly impactful mutation | Explicit approval plus short-lived UAC helper |
| R4 | Irreversible/destructive/unsupported | Refuse or transfer to guided manual procedure |

UCode runs unprivileged. The elevated helper is a separate short-lived process with a signed/hashed request envelope, allow-listed operation id, exact arguments, expiry, nonce, expected snapshot, and one response. It cannot accept free-form model text or become a persistent privileged daemon.

## 8. Shell boundary

Shell is a capability, not an escape hatch.

- `shell.powershell.readonly` accepts a constrained command form, a timeout, and an output cap.
- It rejects mutation-oriented cmdlets/operators, encoded commands, profile loading, nested shells, downloads, and unbounded pipelines.
- Expert mutation shell is a later R3 feature. It requires a displayed command, scope explanation, approval, and a short-lived execution envelope.
- PowerShell, CMD, Git Bash, and WSL are adapters behind one session/receipt model; availability is detected rather than assumed.

## 9. Provider and privacy boundary

Provider requests use one normalized streaming event contract: start, text delta, structured proposal, usage, completion, cancellation, and error.

Before a network provider is used, UCode builds a preview containing:

- provider and endpoint;
- categories to be sent (prompt, selected evidence, file excerpts, diagnostics);
- redaction results and approximate bytes;
- whether paths, usernames, IPs, or secrets are present;
- a per-request allow/cancel decision unless the user has explicitly configured a broader policy.

Secrets remain in Windows Credential Manager. API configuration and provider errors never expose the key in events, logs, exports, or the USB database.

## 10. Repair, verification, and rollback

An action proposal is incomplete unless it defines:

- target and exact intended change;
- evidence supporting the change;
- risk and required authority;
- prerequisites;
- snapshot/backup plan;
- success criteria tied to the original symptom;
- verification probes;
- rollback operation and limits.

Task states distinguish `diagnosed`, `awaiting_approval`, `repaired_unverified`, `verified`, `safe_stop`, `failed`, and `transferred`. Only `verified` is presented as completed repair.

## 11. Coverage model

Domains are delivered at honest levels:

- L1 automated repair: bounded, reversible, independently verifiable.
- L2 guided repair: UCode diagnoses and guides user interaction.
- L3 expert constrained shell: explicit commands and receipts.
- L4 transfer/escalation: collected evidence bundle and next action for a technician/vendor.

Initial domains: system inventory, process/service state, event logs, environment/PATH, networking/DNS/proxy/routes, files/permissions/hashes, and project/runtime diagnostics.

## 12. Desktop experience

The default conversation stays simple. Rescue evidence is progressively disclosed:

- a compact current objective and status;
- a live diagnosis timeline with human-readable probe purpose;
- a hypothesis panel showing supported/excluded/unknown without exposing raw chain-of-thought;
- evidence cards with source, timestamp, and a details drawer;
- one approval card for the proposed repair;
- a final receipt containing before/after evidence and rollback availability.

No dedicated multi-agent tab is required. Specialized workers are an internal execution detail and surface only when they produce useful artifacts.

## 13. Formal acceptance gates

Unit and contract tests must cover schemas, graph invariants, risk classification, redaction, provider boundaries, output limits, cancellation, and persistence.

Fault-injection scenarios must present identical user-visible symptoms with different hidden causes, including:

- wrong DNS, stale proxy, and Hosts override;
- port conflict;
- broken PATH and Python alias/version conflict;
- stopped service;
- corrupted configuration.

Metrics:

- root-cause correctness and top-k hypothesis recall;
- evidence precision and provenance completeness;
- unnecessary probe count and diagnosis latency;
- unauthorized side effects (target: zero);
- false completion (target: zero);
- correct safe-stop/escalation;
- recovery after provider/process interruption;
- offline denial of network providers;
- UI correctness in real packaged desktop flows.

The same symptom must not produce a canned identical diagnosis when evidence differs. Tests inspect both the final answer and the actual machine/fixture state.

## 14. Delivery phases

### Phase 1 — read-only vertical slice

Implement domain types, evidence graph, hypotheses, registry, bounded Windows probes, policy classification, dynamic diagnosis orchestration, persistence/events, desktop evidence UI, and hidden-fault benchmarks. Reuse the existing provider, Super Agent state machine, approval, and receipt infrastructure.

### Phase 2 — reversible repair

Add snapshots and a small allow-listed R2 operation set (for example local config restoration and explicitly scoped environment corrections), independent verification, rollback, and packaged desktop testing.

### Phase 3 — privileged rescue

Add the short-lived elevated helper, authenticated envelopes, R3 operation modules, Windows service/network repairs, and adversarial security tests.

### Phase 4 — breadth and distribution

Expand domain packs, native provider adapters, signed update/model manifests, portable release engineering, benchmark publication, and evidence-bundle export.

Each phase is released only with measured capability boundaries. Documentation must distinguish implemented, tested, experimental, and planned functionality.
