# UCode

UCode is a Windows-first, USB-portable coding task executor. It combines local or user-configured model inference with a host-controlled safety harness: grounded project reading, explicit write approval, transactional file changes, verification evidence, and rollback.

UCode is intentionally narrower than a fully autonomous coding agent. Its reliable target is a clearly scoped task involving roughly one to three files with an explicit validation method.

## Why UCode

- **Portable by design:** source and packaged layouts avoid fixed drive letters; projects can be reconciled after a USB drive moves.
- **Local first:** a verified GGUF model pack can run through the portable runtime; an OpenAI-compatible endpoint is optional.
- **Evidence before confidence:** project answers and completion claims are tied to files, commands, tests, and effects.
- **Approval before writes:** proposed changes are reviewed as a batch and revalidated immediately before application.
- **Failure-safe transactions:** stale files, path escapes, failed writes, and unverified recovery stop the task instead of silently continuing.
- **Honest small-model harness:** deterministic routing, bounded exploration, and policy enforcement stay outside the model.

## Current capabilities

- ordinary chat and code explanation without forcing tool calls;
- bounded project file listing, reading, search, and summarization;
- reviewed single- and multi-file create/modify/delete proposals;
- test execution with factual result capture;
- transactional apply and rollback;
- persistent PowerShell/cmd terminal sessions through ConPTY-compatible PTY support;
- Git status, diff, history, branches, and guarded operations;
- isolated edit-agent worktrees with review before merge;
- local model packs and OpenAI-compatible endpoints;
- dark/light themes, adjustable typography, resizable panels, tray behavior, sounds, and completion notifications.

See [CAPABILITY_BOUNDARY.md](CAPABILITY_BOUNDARY.md) for what is and is not release-qualified.

## Quick development start

Requirements: Node.js 22+, Rust stable with the MSVC target, WebView2, and Windows build tools.

```powershell
npm install
npm run check
Set-Location desktop
npm ci
npm test -- --run
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
```

Run the desktop development shell:

```powershell
Set-Location desktop
npm run tauri dev
```

The repository does not contain model weights or the 2+ GiB portable capsule. See [model-packs/README.md](model-packs/README.md) and [BUILDING.md](BUILDING.md).

## Architecture

The Rust/Tauri desktop is the control plane and persistence authority. A bounded Node executor remains as a compatibility execution layer. UCode borrows lifecycle concepts from DeepSeek's `dsh-superagent-prototype`—trigger, evaluation, decision, mandate, ticket, assignment, effect, report, and delivery—but is not a wholesale copy or a claim of full framework parity.

See [ARCHITECTURE.md](ARCHITECTURE.md).

## Project status

This is a personal/research project and should be evaluated as an evidence-first prototype, not as an unattended production automation system. Automated tests do not replace the physical USB and second-machine acceptance checklist.

## License

UCode source code in this repository is licensed under Apache-2.0. Model weights, runtimes, dependencies, fonts, icons, and sound assets retain their own licenses. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

