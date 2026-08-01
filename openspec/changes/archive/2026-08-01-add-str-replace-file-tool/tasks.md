## 1. Contract and Regression Tests

- [x] 1.1 Add failing registry tests for the `StrReplaceFile` definition, required string
  arguments, optional boolean `replace_all`, and rejection of unknown arguments.
- [x] 1.2 Add failing dispatcher tests for unique replacement, explicit replacement of all
  occurrences, exact replacement counts, and preservation of unrelated file content.
- [x] 1.3 Add failing dispatcher tests proving that absent or ambiguous matches, empty `old`, no-op
  edits, missing or non-regular targets, invalid UTF-8, and oversized files fail without mutation.
- [x] 1.4 Add failing boundary tests for `replace_all` with zero matches, a projected result exactly
  at 4 MiB, output amplification above 4 MiB, and projected-length arithmetic failure; every
  rejected case must prove that the file remains unchanged.
- [x] 1.5 Add a focused instruction-preflight regression proving that `StrReplaceFile.path`
  activates nested `AGENTS.md` scope before the edit executes.

## 2. Core Tool Implementation

- [x] 2.1 Add the provider-visible `StrReplaceFile` JSON Schema definition and map its `path`
  argument to the existing path-based instruction scope policy.
- [x] 2.2 Add a shared 4 MiB text-file boundary and a complete bounded regular UTF-8 read helper
  that rejects oversized input while preserving the existing truncated behavior of `read_file`.
- [x] 2.3 Implement `tools/string_replace.rs` with pre-write validation, unique-match behavior,
  `replace_all`, non-overlapping replacement counting, checked projected-length calculation before
  allocation, the 4 MiB output cap, filesystem error handling, and concise success results.
- [x] 2.4 Route validated `StrReplaceFile` calls through the dispatcher without changing provider
  plugins or moving orchestration into the TUI.

## 3. Documentation and Verification

- [x] 3.1 Update the user-facing tool summary and the relevant core/tool contract documentation
  with `StrReplaceFile` behavior and its deliberate limits.
- [x] 3.2 Run `cargo fmt --all -- --check`, the documented Rust source-width command,
  `cargo clippy --workspace --all-targets -- -D warnings`, focused `StrReplaceFile` tests,
  `cargo test -p misy-core`, and `git diff --check`; fix every failure attributable to the change.
