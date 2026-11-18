# UCode Rescue phase 1 formal acceptance report

Date: 2026-11-18  
Target: Windows 11 x64, packaged USB candidate  
Scope: read-only Rescue vertical slice

## Result

Phase 1 passes its read-only acceptance gate. UCode can route Windows troubleshooting requests away from project coding, collect bounded local evidence, update competing hypotheses, produce an evidence-linked report without model rewriting, and stop before network access, mutation, or elevation.

This result does **not** qualify UCode as a complete repair product. R2 reversible repair, rollback snapshots, the short-lived R3 UAC helper, and broader fault coverage remain later phases.

## Automated gates

| Gate | Result |
|---|---:|
| Node Agent and Executor | 127/127 passed |
| React desktop | 99/99 passed |
| Rust/Tauri host | 125/125 passed |
| TypeScript | passed |
| Vite production build | passed |
| Tauri release build | passed |
| Portable integrity staging | passed |

The Vite build retains a non-blocking chunk-size warning: the main JavaScript asset is approximately 878 KB before gzip and 256 KB after gzip. This is a later startup/maintainability optimization, not a correctness failure.

## Hidden-fault tests

The same visible symptom, “Windows cannot access the network,” was evaluated with different hidden evidence:

| Hidden state | Expected distinction | Result |
|---|---|---|
| IPv4 present but no gateway/default route | support missing-route hypothesis | passed |
| Healthy address/route plus explicit proxy | support proxy hypothesis; contradict missing-route | passed |
| Healthy address, gateway, DNS and no proxy | do not invent a cause; stop before R1 DNS probe | passed |
| Probe access failure | mark evidence unavailable; do not treat it as normal | passed |

Across these fixtures:

- unauthorized mutations: 0;
- automatic R1 network probes: 0;
- false repair completion: 0;
- reports with no evidence graph: rejected;
- identical canned diagnosis across different evidence: 0.

## Real packaged desktop flow

The candidate at `E:\windows-export\UCode Capsule Candidate\UCode.exe` was launched and operated through the native desktop UI using an existing coding conversation to deliberately test history contamination.

User flow:

1. Entered a Windows network diagnosis request in an existing pytest/code conversation.
2. Observed five local read-only probes and one diagnosis decision.
3. Verified the final report referenced evidence IDs and tool IDs.
4. Verified the next probe was `network.dns.resolve` at R1 and was not executed.
5. Verified the receipt showed five verified system evidence items and six technical facts.
6. Verified the receipt showed no system changes and no rollback requirement.
7. Verified the task remained waiting for confirmation/authorization rather than reporting repair completion.

Real-desktop defects found and fixed during this run:

- PowerShell probes initially opened visible black console windows; fixed with `CREATE_NO_WINDOW`.
- Probes initially had no hard deadline; fixed with a 15-second timeout and child termination.
- The small model copied `test_order_total.py` from old project history into a network diagnosis; fixed by host-rendering the evidence graph without model rewriting.
- Rescue probe events initially preceded the follow-up user event, causing the current-turn receipt to show zero facts; fixed by ordering the trigger before probes and adding receipt projection.
- PowerShell output was explicitly set to UTF-8 to preserve localized Windows names.

## Actual machine evidence behavior

On the acceptance machine the report recorded:

- Windows system inventory;
- 54 PATH entries;
- one enabled IP configuration;
- three IPv4 default routes;
- no explicit proxy configuration.

Because local configuration evidence was not sufficient to prove the reported connectivity symptom, UCode selected an R1 DNS resolution probe as the next step and stopped for permission. It did not flush DNS, reset Winsock, modify proxy settings, or claim the network was repaired.

## Remaining product boundary

Phase 1 currently covers broad system/network triage only. It does not yet automatically repair DNS, proxy, routes, services, registry settings, boot failures, drivers, malware, disk corruption, or account/security policy. A real repair release requires Phase 2 snapshots/rollback and Phase 3 authenticated short-lived elevation, each with separate adversarial and desktop acceptance gates.
