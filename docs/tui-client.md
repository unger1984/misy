# TUI Client

## Table of Contents

- [Purpose](#purpose)
- [Client Boundary](#client-boundary)
- [Interaction Model](#interaction-model)
- [Lifecycle and Safety](#lifecycle-and-safety)
- [Change Impact](#change-impact)
- [Sources of Truth](#sources-of-truth)

## Purpose

Read this document before changing terminal rendering, commands, composer behavior, input history,
selection pickers, scrolling, or browser handoff.

## Client Boundary

The TUI is a thin in-process client over `MisyCore`. UI state is explicit, rendering is
deterministic, and keyboard/input is mapped to actions before side effects. Provider, model,
authentication, session, and tool orchestration remain in the core.

## Interaction Model

- The interface uses Ratatui's inline viewport, never the alternate screen. Finalized transcript
  cells are inserted into native terminal scrollback, so mouse selection, copying, and scrollback
  navigation continue to work after Misy exits. Only active response and tool rows are redrawn in
  the inline pane; they move to scrollback as one finalized block.
- The content-driven lower pane is ordered active transcript, optional one-line busy indicator,
  persistent bordered composer, then either a slash-command popup or a modal list, and a footer.
  The popup and modal surface are mutually exclusive; the composer remains visible for both.
- An empty composer renders the dimmed `Ask anything, / for commands` placeholder. The footer
  always shows `? for shortcuts` on the left and the selected provider/model or `model not selected`
  on the right.
- The composer uses a rounded border and an explicit `> ` prompt inside it, supports
  cursor-relative editing and multiline drafts, and keeps bounded submitted-input history.
  `Shift+Enter` inserts a newline and grows the border immediately; `Ctrl+J` is the fallback for
  terminals that cannot report modified Enter. `Up`/`Down` navigate history at text boundaries;
  transcript scrolling belongs to the terminal. The latest 100 accepted prompts and slash commands
  are retained across Misy processes in the user's Misy data directory.
- Typing `/` at the beginning of an empty draft opens a filtered command popup below the composer
  without taking focus from it. `Enter` or `Tab` accepts a command, clears the composer, and opens
  the requested surface; `Esc` dismisses the popup without changing the draft.
- `/provider` opens an interactive provider list. `/model` opens one model list across configured
  providers. Both lists show a dim heading, an empty separator line, eight scrollable numbered
  rows, aligned dim second-column descriptions, and an accent-highlighted selection. Digits select
  visible numbered entries directly. The selected model has a checkmark; provider authentication
  is a second-column status. Providers without credentials are not queried, and a failure from one
  configured provider is shown without hiding models returned by others.
- `Up`/`Down` move, `Enter` accepts, and `Esc` returns. Provider detail renders visible numbered
  `Authorize` or `Log out` actions and `Esc back`; selecting a provider alone has no auth side
  effect.
- Transcript rows use semantic styling: dim user prompts and service messages, normal assistant
  text, structured tool calls with indented results, and red failures. An active submission adds an
  animated one-line spinner with elapsed time and the `esc to interrupt` hint above the composer.

## Lifecycle and Safety

- Opening the provider list does not start every plugin; local manifest/credential state is used
  until a concrete provider action requires the process.
- Browser authorization URLs are validated and passed directly to the OS opener without a shell.
- OAuth completion keeps the opaque provider session and completes through the core without pasted
  credential JSON.
- `Ctrl+C` always cancels active work, shuts down the core/provider host, restores the terminal,
  and exits.
- Event processing is bounded per tick so continuous streaming cannot starve input handling.

## Change Impact

User-visible interaction changes require comparison with both local references and focused
behavior tests. Core events or lifecycle changes also require updates to
[Architecture](architecture.md).

## Sources of Truth

- [`src/tui.rs`](../src/tui.rs)
- [`tests/tui.rs`](../tests/tui.rs)
- [`src/core.rs`](../src/core.rs)
- [Decisions and References](decisions-and-references.md)
