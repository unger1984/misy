# Repository Guidelines

## Project Overview
- `misy` is currently at the bootstrap stage: the repository has Rust tooling configuration but no product source code or tests yet.
- Per the user brief, the project is a fast, minimal agent runtime in Rust, with a focus on speed, simple UX, and extensibility.
- The main interface is TUI. The architecture must also leave a clean path for a separate desktop application on macOS, Linux, and Windows.
- Planned capabilities from the brief: MCP, permissions, and TypeScript plugins via Bun.
- Repository workflow rule: development happens on `dev`, never on `main`.

## Architecture & Data Flow
- The code architecture is not fixed yet; the repository is still empty.
- Practical direction for the first iterations:
  1. The Rust core/runtime accepts commands, config, and manages the agent session.
  2. The TUI frontend is the primary client on top of that core.
  3. The core orchestration layer handles permissions, MCP connections, and plugin execution.
  4. The plugin host invokes TypeScript plugins through Bun.
  5. A future desktop frontend should reuse the same core contract instead of duplicating agent logic.
- Treat this as the working direction, not an implemented fact, until code exists.

## Key Directories
- There are no required directories yet.
- Recommended starter layout if implementation begins:
  - `src/` — Rust core runtime and shared application logic.
  - `src/bin/` — separate entrypoints if the TUI and utility CLI commands diverge.
  - `crates/` — supporting Rust crates if the codebase becomes modular.
  - `ui/tui/` — TUI layer if separated from core.
  - `ui/desktop/` — desktop frontend for macOS/Linux/Windows if a separate app is introduced.
  - `plugins/` — TypeScript/Bun plugins.
  - `tests/` — integration and end-to-end scenarios.
  - `fixtures/` — test inputs and golden outputs.
  - `docs/` — architecture notes and protocols.

## Repository Workflow
- Initialize git before implementation work if the repository is not yet a git repo.
- Create and use the `dev` branch for active work. Do not implement directly on `main`.
- Keep project rule files (`AGENTS.md`) in English.

## Development Commands
- `cargo fmt --all -- --check` — verify Rust formatting.
- `cargo clippy --workspace --all-targets -- -D warnings` — run required Rust lints.
- `cargo test` — run Rust unit and integration tests.

## Code Conventions & Common Patterns
- Conventions are not yet established by code; use the following Rust baseline for new files.
- Keep `main.rs` thin; application and business logic belong in focused modules and explicit state structures, not hidden global mutable state.
- Use `Result` for fallible operations; avoid `unwrap` outside tests, do not silently swallow errors, and document intentional fallbacks in English.
- Avoid unnecessary allocations and copies; prefer borrowing, explicit readable types, and small deliberate public APIs.
- Use `snake_case` for functions/modules and `CamelCase` for types; keep each enum focused on one responsibility.
- Treat the TUI as a client over core state; do not duplicate orchestration or state ownership in the UI.
- Keep side effects at the edges: filesystem, process, network, and environment access should stay out of core logic where possible.
- Rendering should be deterministic and depend only on explicit input state; map input to actions first, then apply actions to state.
- Write comments and docs only where they carry real information; use English, prefer `///` and `//!` for public APIs and reusable contracts, and explain *why*, invariants, cleanup/error-path reasoning, lifecycle constraints, and other non-obvious behavior.
- External integrations must stay capability-scoped and explicit, and new external-facing contracts should remain versionable from the start.
- Tests should prefer observable behavior over implementation details; avoid mocks unless they are genuinely necessary.

## Important Files
- The bootstrap baseline includes `Cargo.toml`, `rust-toolchain.toml`, and `.gitignore`; product source files do not exist yet.
- Once code appears, keep at least these paths current in this document:
  - `Cargo.toml` — workspace/runtime dependencies.
  - `src/main.rs` or `src/bin/*.rs` — TUI/CLI entrypoints.
  - `src/lib.rs` — core exports and shared app contracts.
  - `ui/tui/` — TUI presentation layer, if split out.
  - `ui/desktop/` — desktop app shell, if introduced.
  - `plugins/*/package.json` — plugin runtime and scripts.
  - `tests/` — contract-level verification.

## Runtime/Tooling Preferences
- Facts observed in the workspace: git is initialized and active development happens on the `dev` branch.
- From the user brief:
  - the primary runtime is Rust for the CLI/core;
  - the plugin/runtime layer is TypeScript via Bun;
  - the primary user interface is TUI;
  - desktop GUI support for macOS/Linux/Windows should be a separate client over the same core;
  - when choosing between Node and Bun for plugins, prefer Bun.
- Repository preference: bootstrap git first, create/switch to `dev`, and keep implementation work off `main`.
- Practical rule for assistants: do not introduce Node-only tooling without a clear reason when the task concerns the plugin layer.

## Testing & QA
- There is no test stack in the repository yet.
- For future implementation, keep at least three verification layers:
  - unit tests for the Rust core;
  - integration tests for CLI flows and permissions/MCP;
  - plugin contract tests for Bun/TypeScript adapters.
- For the multi-frontend architecture, contract tests between the core and UI clients are useful so the TUI and desktop app do not diverge in behavior.
- Golden tests for CLI output and scenarios with fake MCP/plugin backends are useful for the agent system.
- Until an existing test suite exists, do not invent custom frameworks unless necessary; first lock down the basic Rust + Bun path.
