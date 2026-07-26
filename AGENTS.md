# Repository Guidelines

## Required Rules

- The architecture is approved. Do not change it unless the user explicitly asks to.
- Work on `dev`, never directly on `main`. Keep project rule files in English.
- Keep `main.rs` thin. Use `Result` for fallible production code and avoid production `unwrap`.
- The Rust core owns orchestration, credentials, sessions, tools, cancellation, and events. Frontends must remain clients of the core.
- Provider plugins are standalone, language-independent subprocess packages. They translate remote protocols and never execute local tools or own Misy credential files.
- Keep external contracts capability-scoped and versioned.

## Documentation Routing

- Read [Architecture](docs/architecture.md) before changing core boundaries, ownership, lifecycle, sessions, tools, or frontend separation.
- Read [Provider Plugins](docs/provider-plugins.md) before changing manifests, discovery, provider processes, JSON-RPC, authentication, models, or streaming.
- Read [TUI Client](docs/tui-client.md) before changing commands, composer behavior, pickers, rendering, keyboard input, or browser handoff.
- Read [Development and Testing](docs/development-and-testing.md) before implementation, verification, or test-structure changes.
- Read [Decisions and References](docs/decisions-and-references.md) before making a new architecture or UX choice. Inspect both local references before inventing a new solution.
- Start at [Documentation Index](docs/README.md) when the task spans multiple areas.
