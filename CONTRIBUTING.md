# Contributing

Contributions should preserve UCode's focused product boundary: portable, local-first, evidence-first, approval before side effects, and safe stop on uncertainty.

## Workflow

1. Open an issue describing the user problem and capability boundary.
2. Add or update tests before changing policy, persistence, routing, or effects.
3. Keep UI activity factual; do not expose private reasoning or raw tool JSON.
4. Run Node, React, Rust, and integration tests described in `BUILDING.md`.
5. Use conventional commit messages such as `fix: preserve structured task context`.

Do not add model weights, generated build output, secrets, user data, or evidence archives. New third-party assets require source and license attribution in `THIRD_PARTY_NOTICES.md`.

