# Capability Boundary

## Release-qualified target

UCode is designed for clear, bounded tasks involving approximately one to three files when the user supplies or accepts a concrete validation method.

Examples:

- explain a named file based on real content;
- locate and summarize a small project area;
- fix a specified function and run a named test;
- create/modify/delete a reviewed three-file batch;
- inspect Git state and work in an isolated worktree;
- run an interactive terminal command with explicit authority.

## Guarded but less reliable

- diagnosis without a file name;
- changes spanning unfamiliar frameworks;
- ambiguous product requirements;
- long chains of dependent edits;
- tasks where the bundled small model must infer architecture from many files.

UCode limits exploration and asks for more evidence instead of silently escalating scope.

## Not supported as a product promise

- unattended end-to-end development of a large application;
- unrestricted shell or filesystem autonomy;
- hidden writes or background commits;
- guaranteed correctness from model output;
- cloud-scale context windows from the portable 1.5B profile;
- physical USB certification based only on automated tests;
- a claim that the included/fetched foundation model was trained by the UCode project.

## Current verification baseline

The repository test suites cover chat/tool routing, streaming, attachments, path safety, transactions, approvals, rollback, provider policy, model pack checks, terminal, Git/worktrees, Super Agent lifecycle, task-context continuity, desktop components, and a live legacy-agent integration fixture.

Success percentages must be reported with the exact model, machine, task set, and failures. Unit test counts are not a substitute for model capability scores.

