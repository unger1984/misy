# TUI Client

## Table of Contents

- [Purpose](#purpose)
- [Client Boundary](#client-boundary)
- [Interaction Model](#interaction-model)
- [Lifecycle and Safety](#lifecycle-and-safety)
- [Change Impact](#change-impact)
- [Sources of Truth](#sources-of-truth)

## Purpose

Read this document before changing terminal rendering, commands, composer behavior, input history, selection pickers, scrolling, or browser handoff.

## Client Boundary

The TUI is a thin in-process client over `MisyCore`. UI state is explicit, rendering is deterministic, and keyboard/input is mapped to actions before side effects. Provider, model, authentication, session, and tool orchestration remain in the core.

## Interaction Model

- The interface is an unboxed, content-flow transcript with concise Misy/provider/model identity, one separator, and a composer directly after current content.
- The composer shows an explicit `› ` prompt and keeps bounded submitted-input history. `Up`/`Down` navigate history and restore the user's draft; transcript scrolling uses separate keys.
- `/provider` opens an interactive provider list. `Up`/`Down` move, `Enter` opens provider detail, and `Esc` returns. Detail offers `Authorize` when logged out or `Log out` when authenticated; selecting a provider alone has no auth side effect.
- `/model` opens an interactive model picker with `Up`/`Down`, `Enter`, and `Esc`.
- Pickers replace the composer temporarily and do not dump catalog rows into the transcript.

## Lifecycle and Safety

- Opening the provider list does not start every plugin; local manifest/credential state is used until a concrete provider action requires the process.
- Browser authorization URLs are validated and passed directly to the OS opener without a shell.
- OAuth completion keeps the opaque provider session and completes through the core without pasted credential JSON.
- `Ctrl+C` always cancels active work, shuts down the core/provider host, restores the terminal, and exits.
- Event processing is bounded per tick so continuous streaming cannot starve input handling.

## Change Impact

User-visible interaction changes require comparison with both local references and focused behavior tests. Core events or lifecycle changes also require updates to [Architecture](architecture.md).

## Sources of Truth

- [`src/tui.rs`](../src/tui.rs)
- [`tests/tui.rs`](../tests/tui.rs)
- [`src/core.rs`](../src/core.rs)
- [Decisions and References](decisions-and-references.md)
