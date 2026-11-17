# Same-model Harness comparison

This public benchmark compares four execution strategies while keeping the model endpoint fixed:

1. `bare`: prompt plus bounded workspace snapshot, no tools.
2. `react`: generic model-owned tool loop.
3. `ucode-single`: UCode deterministic router, policy and verifier with no Scouts.
4. `ucode-team`: the same UCode Harness, with at most two read-only Scouts on selected complex tasks.

The benchmark creates a fresh temporary fixture for every case and group. No action case writes into the source repository. False completion and workspace escape are scored more severely than an explicit safe failure.

## Run

```powershell
npm run benchmark:harness -- --dry-run --cases smoke
npm run benchmark:harness -- --live --coder-url http://127.0.0.1:3284 --cases smoke --output .benchmark-runs/live-smoke
```

Scripted mode tests the benchmark itself; it is never model-quality evidence. Live mode records the model health response, Git commit, timings, per-case results and the Team enablement gate. Generated run directories should remain uncommitted unless they have been reviewed and sanitized.

The automatic Team gate requires at least a 15 percentage-point cross-file success improvement over UCode Single, no increase in false completion, and at most 2× average latency. Simple tasks must not activate Scouts.
