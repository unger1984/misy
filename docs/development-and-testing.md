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
```

## Testing Layers

1. Rust unit tests for domain/core/tool behavior.
2. Rust integration and end-to-end tests for provider host and client flows using fake provider
   processes.
3. Language-native contract tests inside every provider package using fake local remote-provider
   endpoints.

Prefer observable behavior over implementation details. Add concurrency/lifecycle tests where
dropped events, blocked input, leaked processes, or credential exposure are plausible.

## Sources of Truth

- [`Cargo.toml`](../Cargo.toml)
- [`tests/`](../tests/)
- [`plugins/providers/openai/package.json`](../plugins/providers/openai/package.json)
- [`plugins/providers/anthropic/package.json`](../plugins/providers/anthropic/package.json)
- [`plugins/providers/kimi/package.json`](../plugins/providers/kimi/package.json)
- [Architecture](architecture.md)
