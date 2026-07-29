# Rust Code Style

## Table of Contents

- [Purpose](#purpose)
- [Required Checks](#required-checks)
- [Formatting](#formatting)
- [Module Layout](#module-layout)
- [Naming](#naming)
- [Rustdoc](#rustdoc)
- [Error Handling](#error-handling)
- [API Design](#api-design)
- [Concurrency](#concurrency)
- [Lints](#lints)
- [Upstream Sources](#upstream-sources)

## Purpose

Rules for Rust code and tests under `crates/misy-core/` and `crates/misy-tui/`. Read
[Code Style](code-style.md) first — it holds
the language-agnostic rules on size, module boundaries, comments, and error handling. This document
adds what is specific to Rust and does not repeat what is already there.

## Required Checks

Green before a commit that changes Rust source, Rust tests, Cargo inputs, generated Rust, or a
compiled Rust example. Documentation-only and unrelated provider changes do not require these
commands. Use focused tests first; run the full workspace test suite only when the change crosses
Rust crate boundaries or affects shared workspace behavior.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test
```

## Formatting

`rustfmt` decides formatting. Do not argue with it, do not hand-format around it, and do not commit
code it would change. Run `cargo fmt --all` before committing.

The 100-column limit from the general rules is `rustfmt`'s default; do not raise it.

## Module Layout

- `main.rs` stays thin: argument handling and wiring. Everything else lives in `lib.rs` and its
  modules.
- `mod.rs` files are a table of contents: `mod` declarations and re-exports only. Do not put type,
  function, `impl`, or `trait` definitions in `mod.rs`. Name each file after its primary definition
  so the file tree mirrors the module tree. This is the rule behind Clippy's
  `definition_in_module_root` restriction lint.
- Prefer `pub(crate)` and `pub(super)` over `pub` for anything that is not part of the crate's
  public contract.
- Re-export the public contract from `lib.rs` so clients depend on one path, not on internal module
  structure.

## Naming

Follow RFC 430 — `rustfmt` and Clippy assume it:

| Item | Convention |
| --- | --- |
| Modules, functions, methods, locals | `snake_case` |
| Types, traits, enum variants | `UpperCamelCase` |
| Constants and statics | `SCREAMING_SNAKE_CASE` |
| Type parameters | short `UpperCamelCase`, usually `T` |
| Lifetimes | short lowercase, `'a`, `'src` |

- Acronyms count as one word: `Uuid`, not `UUID`; `Stdin`, not `StdIn`.
- Conversions follow the cost convention: `as_` is free (borrowed → borrowed), `to_` is expensive,
  `into_` consumes the receiver.
- Getters are named after the field, without a `get_` prefix.
- Constructors are `new`, or `with_...` when they take configuration.

## Rustdoc

- Every public item carries `///`; every module carries `//!`.
- Document which errors a function returns and when (`clippy::missing_errors_doc`).
- Document anything that can panic — `expect`, `unwrap`, slice indexing, arithmetic overflow —
  and state why it cannot happen in practice (`clippy::missing_panics_doc`).
- Use intra-doc links (`[`ProviderHost`]`) rather than naming items in plain prose. RFC 1574 calls
  this "link all the things".
- Crate-level docs in `lib.rs` explain what the crate is and how a client uses it.

## Error Handling

- Fallible operations return `Result`. No sentinel values, no bare `bool`.
- **No `unwrap()` in production code.** `clippy::unwrap_used` is denied outside tests.
- `expect()` is permitted only where the invariant is local and provable, and the message states
  the invariant, not the symptom: `expect("history mutex must not be poisoned")`, never
  `expect("failed")`.
- Error enums implement `Debug`, `Display`, and `std::error::Error`, with `source()` wired up so
  callers can walk the chain.
- Give the error type variants when callers must distinguish cases. Do not match on message text.
- `?` for propagation. Do not wrap a `Result` in a `match` only to re-return it.

## API Design

- Types eagerly implement the common traits where applicable: `Debug`, `Clone`, `PartialEq`, and
  `Serialize`/`Deserialize` for anything that crosses a process, plugin, or client boundary
  (`C-COMMON-TRAITS`, `C-SERDE`).
- Prefer borrowing over cloning; prefer moving over copying large values. Do not clone to escape a
  borrow error — restructure.
- Accept the most general parameter that works: `&str` over `&String`, `impl AsRef<Path>` over
  `&PathBuf`, where it does not complicate the signature.
- Newtypes over bare primitives for domain identifiers. `ProviderId` and `ModelId` exist for this
  reason; do not pass a raw `String` where a domain type exists.
- Make illegal states unrepresentable: an enum with meaningful variants beats a struct of optional
  fields plus a convention.
- Public contracts crossing a boundary are versioned from the start and capability-scoped.

## Concurrency

- State a lock's scope and purpose in a comment where it is declared. A reader must be able to
  determine lock ordering without running the program.
- Never hold a lock across a blocking call — I/O, a subprocess request, a channel receive — unless
  that is the explicit intent. If it is, say so in a comment and bound it with a timeout.
- Every blocking wait has a deadline. An unbounded `recv()` on a channel fed by an external process
  will eventually hang the program.
- Do not poll in a loop with `thread::sleep` to work around the absence of a combined wait. Merge
  the inputs into one channel and block on that.
- Bounded channels drop; unbounded channels grow. Choose deliberately and document which property
  you needed.
- `unsafe` is forbidden. If you believe you need it, raise it first.

## Lints

Configure in `Cargo.toml` and keep the build green under it:

```toml
[lints.rust]
missing_docs = "warn"
unsafe_code = "forbid"

[lints.clippy]
# Cherry-picked from pedantic/restriction/nursery. Clippy's own guidance is to
# pick individual lints rather than enable whole groups.
too_many_lines = "deny"
cognitive_complexity = "deny"
unwrap_used = "deny"
missing_errors_doc = "warn"
missing_panics_doc = "warn"
doc_markdown = "warn"
needless_pass_by_value = "warn"
redundant_clone = "warn"
module_name_repetitions = "warn"
```

Enable them one at a time; see [Enforcement Philosophy](code-style.md#enforcement-philosophy).

`#[allow(...)]` must be narrow and must carry a comment explaining why the lint is wrong here.

## Upstream Sources

- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) — public API design; the
  [checklist](https://rust-lang.github.io/api-guidelines/checklist.html) is the short form.
- [Rust Style Guide](https://doc.rust-lang.org/style-guide/) — formatting, as implemented by
  `rustfmt`.
- [RFC 430](https://github.com/rust-lang/rfcs/blob/master/text/0430-finalizing-naming-conventions.md)
  — naming conventions.
- [RFC 1574](https://github.com/rust-lang/rfcs/blob/master/text/1574-more-api-documentation-conventions.md)
  — documentation conventions.
- [Clippy lint list](https://doc.rust-lang.org/clippy/lints.html) and
  [Clippy usage](https://doc.rust-lang.org/stable/clippy/usage.html).

Where this document and an upstream guideline conflict, raise it rather than silently choosing.
