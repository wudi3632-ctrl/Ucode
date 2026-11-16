# Security Policy

## Reporting

Please report security issues privately to the repository owner rather than opening a public issue containing secrets, exploit details, or user project data. Until a dedicated security address is published, use GitHub's private vulnerability reporting feature when available.

## Security model

UCode treats model output as untrusted. Local policy validates routes, authority, canonical paths, effect digests, commands, provider endpoints, model manifests, and rollback state.

Important boundaries:

- select only projects you are willing to expose to the configured model provider;
- network providers are opt-in and receive the context shown in settings;
- API keys are stored through Windows Credential Manager;
- terminal and Git operations can change a project and require explicit user action;
- model packs must match their manifest size and SHA-256 digest;
- source releases contain no model weights, runtime binaries, user databases, logs, or evidence archives.

## Before publishing a release

Run tests, review dependency advisories, scan tracked files for credentials, reject oversized binary artifacts, verify third-party notices, and inspect the final Git index. Never commit `.env`, SSH keys, remote passwords, user workspaces, or database files.

