# Repository Guidelines

## Required Rules

- The architecture is approved. Do not change it unless the user explicitly asks to.
- Work on `dev`, never directly on `main`. Keep project rule files in English.
- Keep `crates/misy-tui/src/main.rs` thin. Use `Result` for fallible production code and avoid
  production `unwrap`.
- The Rust core owns orchestration, credentials, sessions, tools, cancellation, events, the agent
  tree, and background-agent lifecycle. Frontends must remain clients of the core.
- Provider plugins are standalone, language-independent subprocess packages. They translate remote protocols and never execute local tools or own Misy credential files.
- Keep external contracts capability-scoped and versioned.
- Follow [Code Style](docs/code-style.md): size limits, module boundaries, `why`-not-`what`
  comments, documented public APIs. It is binding for every language in the tree.
- Verification is impact-scoped. Run a formatter, linter, compiler, or test suite only when the
  change modifies inputs or behavior that the check can validate. Documentation-only changes do
  not require Rust or TypeScript checks unless they alter compiled examples, generated artifacts,
  or executable contracts. Do not rerun unaffected language or provider suites.

## Documentation Routing

- Read [Architecture](docs/architecture.md) before changing `misy-core` boundaries, ownership,
  lifecycle, sessions, tools, or frontend separation.
- Read [Provider Plugins](docs/provider-plugins.md) before changing manifests, discovery, provider processes, JSON-RPC, authentication, models, or streaming.
- Read [TUI Client](docs/tui-client.md) before changing commands, composer behavior, pickers, rendering, keyboard input, or browser handoff.
- Read [Code Style](docs/code-style.md) before writing or reviewing any code, then the document for the language you are touching: [Rust](docs/code-style-rust.md) or [TypeScript](docs/code-style-typescript.md).
- Read [Development and Testing](docs/development-and-testing.md) before implementation, verification, or test-structure changes.
- Read [Decisions and References](docs/decisions-and-references.md) before making a new architecture or UX choice. Inspect both local references before inventing a new solution.
- Start at [Documentation Index](docs/README.md) when the task spans multiple areas.
