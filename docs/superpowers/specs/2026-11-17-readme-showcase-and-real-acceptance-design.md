# UCode README Showcase and Real Acceptance Design

## Goal

Make the public README immediately communicate what UCode is, how it differs from an ordinary chat UI, and which claims were verified in the packaged Windows desktop application.

## README structure

1. A concise product statement and status badges.
2. One real desktop hero screenshot showing a grounded project conversation and task context.
3. A three-step story: ask, review, verify.
4. Two supporting real screenshots covering reviewed Diff and the terminal/Git/Agents work surfaces.
5. A compact capability table that distinguishes verified behavior from current limits.
6. Architecture, development setup, release boundary, and licensing links.

Screenshots live in `docs/assets/readme/`, use descriptive filenames and alt text, and must come from the final packaged `UCode.exe`. They must not expose credentials, private projects, or unnecessary machine paths. Large GIFs are excluded from this iteration to keep GitHub loading fast.

## Real acceptance matrix

- Multi-turn chat: exact answer, follow-up memory, ordinary explanation, and no historical-message restream.
- Grounded project reading: existing file, missing file, Markdown code block, and traversal rejection.
- Reviewed mutation: proposed file creation or edit, no pre-approval write, visible Diff, approval, pytest evidence, and completion receipt.
- Recovery behavior: rejection or cancellation leaves files unchanged; packaged runtime remains healthy across restart and manifest refresh.
- Desktop interaction: side panels, theme, resize behavior, and Start/Diff/Files/Terminal/Git/Agents surfaces remain reachable.

## Success criteria

- A new GitHub reader can explain UCode's purpose, safety model, and practical boundary from the first screen of the README.
- Every prominent capability claim has either a real screenshot or a recorded test result.
- The packaged capsule passes its manifest verifier, automated suites, live Agent integration, and the real desktop interaction matrix.
- Any incomplete or model-limited behavior is described explicitly rather than presented as finished.
