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

- The interface uses Ratatui's fullscreen alternate screen. The complete conversation remains in
  application-owned render state while Misy is open, with the newest transcript rows above the
  composer. Leaving Misy restores the terminal screen and scrollback that existed before startup.
- The fullscreen layout is ordered transcript, optional one-line busy indicator,
  persistent bordered composer, then either a slash-command popup or a modal list, and a footer.
  The popup and modal surface are mutually exclusive; the composer remains visible for both.
- A responsive startup card is the first item in the transcript flow. It shows the Misy version,
  initial model, working directory, and brief input hints; it scrolls off the top with earlier
  conversation content and is never a persistent header.
- An empty composer renders the dimmed `Ask anything, / for commands` placeholder. The footer
  always shows `? for shortcuts` on the left and the selected provider/model or `model not selected`
  on the right.
- The composer uses a rounded border and an explicit `> ` prompt inside it, supports
  cursor-relative editing and multiline drafts, and keeps bounded submitted-input history.
  `Shift+Enter` inserts a newline and grows the border immediately; `Ctrl+J` is the fallback for
  terminals that cannot report modified Enter. `Up`/`Down` navigate history at text boundaries.
  The latest 100 accepted prompts and slash commands are retained across Misy processes in the
  user's Misy data directory.
- A plain left click in composer text moves its cursor. Dragging across any visible Misy content
  renders an application-owned selection; releasing the mouse sends the selected text over OSC 52
  and immediately clears the highlight. This lets a terminal host such as Herdr own clipboard
  access and display its normal copy feedback. Bracketed paste and forwarded `Cmd+V` insert text
  atomically at the cursor and never submit embedded newlines.
- Typing `/` at the beginning of an empty draft opens a filtered command popup below the composer
  without taking focus from it. The popup shows at most eight commands; `Up` and `Down` scroll its
  window and wrap between the first and last matching commands. `Tab` completes the selected
  command with its canonical name and a trailing space, then hides the popup without executing it.
  `Enter` executes the selected or completed command; `Esc` dismisses the popup without changing
  the draft.
- `/usage` fetches the selected model provider's account limits in the background and appends a
  normalized, display-safe usage report to the transcript. It takes no arguments. The report shows
  provider-wide and per-limit notes, consumed and remaining amounts, exhausted limits, and known
  reset timing. If no model is selected, the provider does not declare usage capability version 1,
  authentication fails, the request times out, or the report is invalid, the TUI renders the error
  without changing the selected model.
- `/exit` takes no arguments and exits through the same cancellation, provider shutdown, and
  terminal-restoration path as `Ctrl+C`.
- `/provider` opens an interactive provider list in the lower panel. `/model` immediately opens a
  centered model popup from the core-owned catalog cache while refreshing the catalog in the
  background. An empty cache shows an animated loading indicator. The popup has an `All` tab and
  one tab for every provider represented by models or an error; `Left` and `Right` switch tabs and
  `Up` and `Down` move within the filtered model list. Its width is capped and centered on wider
  terminals, while provider tabs wrap across rows and keep the active tab visible on short screens.
  The model viewport uses the height left below those rows and scrolls the selection when the list
  does not fit. The text filter is retained across tabs, and a background refresh preserves the
  active tab and selected model when they still exist. Lists show up to eight scrollable numbered
  rows, aligned dim second-column descriptions, and an
  accent-highlighted selection. Digits select visible numbered entries directly. The selected
  model has a checkmark; provider authentication is a second-column status. Providers without
  credentials are not queried, and a failure from one configured provider is shown without hiding
  models returned by others.
- `Up`/`Down` move, `Enter` accepts, and `Esc` returns. Provider detail renders visible numbered
  `Authorize` or `Log out` actions and `Esc back`; selecting a provider alone has no auth side
  effect.
- Transcript rows use semantic styling: dim user prompts and service messages, normal assistant
  text, structured tool calls with indented results, and red failures. An active submission adds an
  animated one-line spinner with elapsed time and the `esc to interrupt` hint above the composer.
- The first accepted prompt is shown optimistically in the transcript. Prompts submitted while a
  turn is active remain in a bounded queue preview above the composer and enter the transcript only
  when the core starts them. The core processes them in FIFO order, so each answer stays directly
  after its prompt. Before the first assistant text the activity row says `Thinking…`; once text
  begins it says `Responding…`.

## Lifecycle and Safety

- Opening the provider list does not start every plugin; local manifest/credential state is used
  until a concrete provider action requires the process.
- Provider authentication follows the `auth.start` kind. `browser` validates and opens the URL,
  then waits for provider completion. `device` does the same while showing the user code in the
  provider operation row. `none` immediately marks the provider authenticated without opening a
  browser. `prompt` reports that field input is not supported by this client yet.
- Browser and device authorization URLs are limited to validated HTTP(S) addresses and are passed
  directly to the OS opener without a shell. Unknown authentication kinds are reported as errors.
- Authentication completion keeps the opaque provider session separate from the empty browser or
  device completion object and passes both through the core without pasted credential JSON.
- During active work, `Ctrl+C` interrupts the current operation without exiting. While idle, the
  first press highlights `press Ctrl+C again to exit` in the footer for one second; a second press
  inside that window shuts down the core/provider host, restores the terminal, and exits. Any
  other input clears the armed shortcut. `/exit` performs the same clean shutdown immediately.
- Event processing is bounded per tick so continuous streaming cannot starve input handling.
- `Esc` interrupts the active turn. When prompts are queued, the next one starts after cancellation
  and the activity row briefly offers `esc again to stop queue`; a second press within one second
  cancels both the active turn and every remaining queued submission. With no queued prompt, one
  press is sufficient to stop the active turn.
- Usage is a core operation, not a TUI-owned provider request: the TUI supplies no endpoint,
  headers, credentials, or provider-specific parsing.

## Change Impact

User-visible interaction changes require comparison with both local references and focused
behavior tests. Core events or lifecycle changes also require updates to
[Architecture](architecture.md).

## Sources of Truth

- [`src/tui.rs`](../src/tui.rs)
- [`tests/tui.rs`](../tests/tui.rs)
- [`tests/tui_model_popup.rs`](../tests/tui_model_popup.rs)
- [`src/core.rs`](../src/core.rs)
- [Decisions and References](decisions-and-references.md)
