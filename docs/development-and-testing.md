# Development and Testing

## Table of Contents

- [Purpose](#purpose)
- [Workflow](#workflow)
- [Required Checks](#required-checks)
- [Testing Layers](#testing-layers)
- [Sources of Truth](#sources-of-truth)

## Purpose

Read this document before implementation, verification, or changes to the test structure.

## Workflow

- Active development happens on `dev`, never directly on `main`.
- Preserve unrelated work in a dirty tree and commit only owned files.
- Use test-first development for behavior changes and keep side effects behind testable boundaries.
- Provider tests must use local fake backends; automated tests must not require real provider
  credentials or external network access.

## Required Checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test
cd plugins/providers/openai
bun test
bunx tsc --noEmit
cd ../anthropic
bun test
bunx tsc --noEmit
cd ../kimi
bun test
bunx tsc --noEmit
cd ../_sdk
bun test
bunx tsc --noEmit
```

## Testing Layers

1. Rust unit tests live beside the implementation in `crates/misy-core/src/` and
   `crates/misy-tui/src/`.
2. Rust integration and end-to-end tests live in `crates/misy-core/tests/` for core, provider-host,
   configuration, credential, domain, and tool flows, and in `crates/misy-tui/tests/` for terminal
   client flows. They use fake provider processes from `crates/misy-core/tests/fixtures/`.
   Integration tests reach core internals (`ProviderHost`, `ProviderCatalog`, `CredentialStore`,
   tool types, deadline-tuning constructors) through the crate's `test-support` feature, enabled
   by a self dev-dependency in `crates/misy-core/Cargo.toml`; production clients never see them.
3. Language-native contract tests inside every provider package use fake local remote-provider
   endpoints.

Prefer observable behavior over implementation details. Add concurrency/lifecycle tests where
dropped events, blocked input, leaked processes, or credential exposure are plausible.

## Sources of Truth

- [`Cargo.toml`](../Cargo.toml)
- [`crates/misy-core/tests/`](../crates/misy-core/tests/)
- [`crates/misy-tui/tests/`](../crates/misy-tui/tests/)
- [`plugins/providers/openai/package.json`](../plugins/providers/openai/package.json)
- [`plugins/providers/anthropic/package.json`](../plugins/providers/anthropic/package.json)
- [`plugins/providers/kimi/package.json`](../plugins/providers/kimi/package.json)
- [`plugins/providers/_sdk/package.json`](../plugins/providers/_sdk/package.json)
- [Architecture](architecture.md)
