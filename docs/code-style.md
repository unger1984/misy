# Code Style

## Table of Contents

- [Purpose](#purpose)
- [Language-Specific Rules](#language-specific-rules)
- [Non-Negotiables](#non-negotiables)
- [File and Function Size](#file-and-function-size)
- [Module Organization](#module-organization)
- [Comments](#comments)
- [Documentation](#documentation)
- [Naming](#naming)
- [Error Handling](#error-handling)
- [Enforcement Philosophy](#enforcement-philosophy)

## Purpose

Read this document before writing or reviewing any code in this repository. It holds the rules that
apply to **every language in the tree**. Language-specific rules live in their own documents, linked
below, and are equally binding.

These rules exist because the codebase has already drifted: a 1200-line UI module, a provider
adapter with several statements crammed onto single 160-column lines, and functions that mix
transport, parsing, and policy in one body. That is the state these documents forbid.

Rules are binding. Where a rule is a judgment call rather than an absolute, it says so.

## Language-Specific Rules

- [Rust Code Style](code-style-rust.md) — the core and terminal-client workspace crates under
  `crates/`.
- [TypeScript Code Style](code-style-typescript.md) — provider plugin packages under `plugins/`.

When a language document and this one disagree, the language document wins for that language: it is
more specific and tool-backed. Anything not covered there falls back to the rules here.

## Non-Negotiables

Every check required by the language documents must be green before any commit. A failing check is
never "someone else's problem" and never "out of scope" — fix it when you see it.

## File and Function Size

Size limits are a proxy for a real problem: a unit that does more than one thing cannot be reviewed,
tested, or reasoned about as a whole.

**Files**

- Target: **under 300 lines**.
- Soft limit: **500 lines**. Crossing it requires a one-line justification in the commit message.
- Hard limit: **700 lines**. Do not merge a source file above this. Split it.

When a file grows past the target, the fix is almost never "delete comments". It is that the file
holds several responsibilities that want to be separate modules.

**Functions**

- Target: **under 50 lines**; hard ceiling **100**.
- A function that needs a section comment (`// --- parse ---`) is telling you it should be two
  functions.

**Lines**

- Maximum width **100 columns**.
- **One statement per line.** Chaining several statements onto one line to reduce the line count is
  explicitly forbidden. It defeats diffs, blame, and debuggers, and it is the single most common
  form of fake concision in this repository.

**Nesting**

- Maximum **4 levels** of block nesting inside a function. Prefer early return and guard clauses
  over an arrow of nested conditionals.

## Module Organization

- Each entry point stays thin: argument handling and wiring only. Logic lives in libraries.
- One module owns one responsibility. If you cannot name a module's responsibility in a single
  sentence without "and", split it.
- Keep the public surface small and deliberate. Anything not part of the public contract is
  private; widen visibility only when a caller genuinely needs it.
- Side effects — filesystem, process spawning, network, environment, clock — live at the edges.
  Core logic takes its inputs as parameters so it can be tested without a sandbox or a network.
- Do not duplicate a concept across modules. Two copies of the same list, schema, or constant will
  diverge silently; derive one from the other or share a single definition.

## Comments

The rule is **why, not what**. A comment restating the code is noise; a comment explaining a
decision is often the most valuable thing in the file.

**Write a comment when, and only when, the code cannot say it itself:**

- Why this approach and not the obvious alternative.
- Invariants a reader must not break.
- Lifecycle and ordering constraints ("this lock is held across the whole read-modify-write because
  otherwise a second process can interleave").
- Error and cleanup paths, and what happens on the failure branch.
- References to an external contract: a protocol field, an upstream quirk, a spec section.
- Deliberate deviations from an obvious best practice, with the reason.

**Do not write:**

- Comments that restate the next line.
- Commented-out code. Delete it; git remembers.
- `TODO` without an owner and a concrete condition for its removal.

## Documentation

- Every public item — module, type, function, exported symbol — carries a doc comment. This is not
  optional for anything another component is written against.
- Module-level docs explain the module's responsibility and its place in the system, top down. A
  reader should understand what a module is for without reading its code.
- Document failure: which errors a function produces and under which conditions.
- Document anything that can abort the process, and say why it cannot happen in practice.
- Prefer a link to a related item over naming it in prose.

## Naming

Names carry meaning. `data`, `value`, `result`, `handle`, `manager`, `util` as a primary name are
banned unless the thing genuinely is that generic. A reader must be able to tell what a binding
holds without tracing its origin.

Use the naming conventions native to the language — see the language documents. Do not import
another language's casing or prefixes.

## Error Handling

- Failures are values or exceptions, per the language's idiom — never sentinel returns or booleans
  that lose the reason.
- Never silently swallow an error. If a failure is deliberately ignored, say so in a comment with
  the reason.
- Preserve the cause. A caller must be able to walk the chain to the original failure.
- Error messages describe what the system could not do and with what, not internal state. Users
  read them.
- Do not use error message strings as control flow. If a caller must distinguish cases, give the
  error a type or a code.
- Every blocking wait has a deadline. An unbounded wait on something an external process controls
  is a hang waiting to happen.

## Language

All code, comments, documentation, identifiers, and commit messages are in **English**, regardless
of the language used in conversation or in task briefs under `.ai/`.

## Enforcement Philosophy

- Prefer a machine-checked rule over a written one. A rule nobody can verify is a suggestion.
- Adopt new lints **incrementally**: enable one, fix what it reports, commit, move to the next.
  Enabling everything at once and then blanket-suppressing the fallout produces the appearance of
  enforcement without the substance.
- A suppression (`#[allow]`, `// biome-ignore`, `@ts-expect-error`) is permitted only for a genuine
  false positive. It must be as narrow as possible and must carry a comment stating why the rule is
  wrong here. A bare suppression is a review blocker.
