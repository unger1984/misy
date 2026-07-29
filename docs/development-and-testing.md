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

Required checks are selected by impact; this is not a command list to run after every change.
Run a check only when the change modifies an input, generated artifact, or behavior that the check
can validate. Start with focused tests, then widen to the affected package or workspace boundary.

- Rust source, Rust tests, Cargo manifests, or shared Rust contracts: run `cargo fmt` and Clippy,
  plus focused tests for the changed behavior. Run the full Rust workspace only for shared-crate,
  workspace-configuration, or cross-crate changes.
- A provider package: run that package's typecheck, tests, and Biome check. Run other providers
  only when a shared SDK or wire contract they consume changed.
- Documentation-only changes: do not run Rust or TypeScript suites unless the documentation
  contains compiled examples, drives generated output, or changes an executable contract covered
  by those suites. `git diff --check` remains relevant because it validates the changed text.
- Mixed changes: combine only the checks required by the affected areas. A later documentation or
  changelog edit does not invalidate already completed code checks.

Canonical commands, when their area is affected:

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

One source-code rule is not covered by the tools above and needs its own check when Rust or
TypeScript source changed.

`rustfmt` leaves macro bodies alone and Biome does not wrap single-line comments, so the 100-column
limit from [Code Style](code-style.md#file-and-function-size) can be broken while `cargo fmt
--check` and `biome check` stay green:

```bash
find crates -name '*.rs' -exec awk 'length>100 {print FILENAME":"FNR}' {} \;
find plugins/providers -path '*/node_modules' -prune -o -name '*.ts' -print \
  | xargs awk 'length>100 {print FILENAME":"FNR}'
```

## Testing Layers

1. Rust unit tests live beside the implementation in `crates/misy-core/src/` and
   `crates/misy-tui/src/`.
2. Rust integration and end-to-end tests live in `crates/misy-core/tests/` for core, provider-host,
   configuration, credential, session, domain, and tool flows, and in `crates/misy-tui/tests/` for
   terminal client flows. They use fake provider processes from
   `crates/misy-core/tests/fixtures/`.
   Integration tests reach core internals (`ProviderHost`, `ProviderCatalog`, `CredentialStore`,
   tool types, deadline-tuning constructors) through the crate's `test-support` feature, enabled
   by a self dev-dependency in `crates/misy-core/Cargo.toml`; production clients never see them.
3. Language-native contract tests inside every provider package use fake local remote-provider
   endpoints.

Prefer observable behavior over implementation details. Add concurrency/lifecycle tests where
dropped events, blocked input, leaked processes, or credential exposure are plausible.
Session tests use an injected `MisyPaths` root and must cover lazy creation, private permissions,
append/resume round trips, malformed tails, schema rejection, cwd filtering, model fallback, and
the active-submission switch guard without reading the user's real session directory.

## Sources of Truth

- [`Cargo.toml`](../Cargo.toml)
- [`crates/misy-core/tests/`](../crates/misy-core/tests/)
- [`crates/misy-tui/tests/`](../crates/misy-tui/tests/)
- [`plugins/providers/openai/package.json`](../plugins/providers/openai/package.json)
- [`plugins/providers/anthropic/package.json`](../plugins/providers/anthropic/package.json)
- [`plugins/providers/kimi/package.json`](../plugins/providers/kimi/package.json)
- [`plugins/providers/_sdk/package.json`](../plugins/providers/_sdk/package.json)
- [Architecture](architecture.md)
