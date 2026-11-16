# Building UCode

## Supported development host

The current desktop target is Windows 11 x64 with WebView2. Development requires:

- Node.js 22.19 or newer;
- Rust stable (`x86_64-pc-windows-msvc`);
- Visual Studio Build Tools with the Windows SDK;
- WebView2 Runtime;
- Git for Git/worktree features;
- Python only for Python project tests (not for building the shell).

## Source verification

```powershell
npm install
npm run check

Set-Location desktop
npm ci
npm test -- --run
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
```

## Desktop build

```powershell
Set-Location desktop
npm run build:portable
```

The Tauri executable is written below `desktop/src-tauri/target/release`. Build output is intentionally ignored by Git.

## Portable capsule

A distributable capsule is assembled outside the source repository. Its expected top-level layout is:

```text
UCode/
  UCode.exe
  runtime/
  model-packs/
  data/
  workspaces/
```

The runtime and model files have separate licenses and are not produced by a source checkout. A release process must generate a file manifest and SHA-256 digest list, then verify the assembled capsule before publishing.

## Physical acceptance

Automated builds do not prove portability. Before marking a release portable, manually verify:

- launch and normal use from a real removable drive;
- a changed drive letter;
- a second clean Windows user/machine;
- standard-user privileges;
- offline startup;
- no preinstalled Node, Python, Rust, or Git (except features that explicitly require Git/Python);
- Defender and SmartScreen behavior;
- WebView2 absence/degraded messaging;
- safe behavior after unexpected drive removal.

Unperformed checks must remain marked as unverified in release notes.

