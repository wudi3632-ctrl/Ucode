# Third-Party Notices

This document describes important upstream projects and model families used or referenced by UCode. It is not a replacement for the license files distributed with each binary or model artifact.

## Source dependencies

- Tauri and Tauri plugins — Apache-2.0/MIT, according to their upstream packages.
- React and related packages — MIT.
- xterm.js — MIT.
- Rust crates and npm dependencies — each retains the license declared in `Cargo.lock` or `package-lock.json` and its upstream package.
- llama.cpp runtime — MIT; distributed runtime packages must include its notices.

## Models

- Qwen2.5-Coder model packs are third-party model artifacts. Their applicable Qwen license and source attribution must ship with the pack. UCode does not claim to have trained Qwen.
- MiniMind-related research/controller material, when used, retains its upstream ownership and license. The public source UI does not claim a MiniMind process is installed or active unless a verified runtime reports it.

Model weights are intentionally excluded from this Git repository. A model pack publisher is responsible for verifying redistribution rights, including the exact quantization source, before release.

## Architecture reference

UCode's lifecycle design was informed by DeepSeek's open-source `dsh-superagent-prototype` project. UCode adapts concepts such as Trigger, Evaluation, Decision, Mandate, Ticket, Assignment, Effect, Report, and Delivery to a smaller local desktop control plane. It is not a full upstream port. Preserve the upstream MIT notice when redistributing copied or derived upstream code.

Reference: https://github.com/deepseek-ai/dsh-superagent-prototype

## Design/reverse-engineering tools

REA and Craft Anything were used as development aids for inspection and comparison. They are not embedded in, required by, or redistributed with UCode unless a future release explicitly states otherwise.

## Assets

The application currently uses system fonts (Segoe UI Variable, Microsoft YaHei UI, Cascadia Code fallbacks) and project-owned/generated UI icons and tones. Any future external icon, font, or sound asset must be recorded here with its source and license before release.

