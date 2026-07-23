# Task 3 report

## Scope
- Files changed: `Cargo.toml`, `Cargo.lock`, `src/tui.rs`
- Added dependency: `arboard = "3.6.1"`
- No full `cargo test` run

## RED
Brief command `cargo test paste_ composer_key_events bracketed_paste` is not valid Cargo syntax for multiple filters in one invocation. Used the same filters as three targeted RED runs chained in order:

```bash
cargo test paste_ && cargo test composer_key_events && cargo test bracketed_paste
```

Initial RED result:
- missing `ClipboardImage`
- missing `ClipboardError`
- missing `paste_action`
- missing `composer_action_from_key`
- missing `handle_terminal_event`
- missing bracketed paste lifecycle state/cleanup step

This proved tests were failing for missing Task 3 behavior, not for unrelated assertions.

## Implemented behavior

### Terminal lifecycle
- `EnableBracketedPaste` added during setup after mouse capture.
- `SetupState` now tracks `bracketed_paste_enabled`.
- `CleanupStep` now includes `DisableBracketedPaste`.
- `restoration_plan` places `DisableBracketedPaste` before `DisableRawMode`.
- `restore_terminal` executes symmetric disable step without unwrap.

### Clipboard boundary
- Added TUI-local boundary types:
  - `ClipboardImage { width, height, rgba }`
  - `ClipboardError`
- `clipboard_image()`:
  - creates `arboard::Clipboard`
  - maps `ContentNotAvailable` to `Ok(None)`
  - maps all other errors to `Err(ClipboardError)`
  - moves RGBA into owned `Vec<u8>` once with `image.bytes.into_owned()`
- `clipboard_text()` follows the same `ContentNotAvailable -> Ok(None)` contract.
- `paste_action()` enforces image-first policy and aborts on any error.

### Event routing
- Added `handle_terminal_event()` for centralized TUI event routing.
- `Event::Paste(String)` inserts text only when `FocusedPane::Composer`.
- `Ctrl-V` while composer is focused:
  - tries clipboard image first
  - only reads text if image is absent
  - keeps draft unchanged on error
  - writes concise notification into `ViewState.notification`
- Non-composer key behavior remains routed through existing `action_from_key_event`.

### Composer key routing
- Added `composer_action_from_key(KeyEvent) -> Option<Action>`.
- Implemented:
  - printable chars without Ctrl/Alt -> `InsertText`
  - Left/Right -> cursor actions
  - Backspace/Delete
  - PageUp/PageDown and Ctrl-Up/Ctrl-Down -> composer scroll
  - Enter -> `SubmitComposer`
  - Shift+Enter -> `InsertLineBreak`
  - Up/Down -> no-op in composer
- `Ctrl-V` is intercepted in `handle_terminal_event()` before generic mapping.

### Platform helper comments
Included exact required comments:

```rust
#[cfg(target_os = "linux")]
// TODO(linux): implement OSC 5522 image paste
#[cfg(target_os = "windows")]
// TODO(windows): implement OSC 5522 image paste
```

### Required TODO comment
Added exact task-scoped TODO for the current notification shortcut:

```rust
// TODO(task-3): route clipboard notifications through a shared status area when TUI gets one.
```

## Tests added in `src/tui.rs`
1. `composer_key_events_map_enter_and_shift_enter_to_distinct_actions`
2. `paste_action_prefers_clipboard_image_over_text`
3. `paste_action_uses_clipboard_text_when_image_is_absent`
4. `paste_action_returns_error_without_text_fallback_or_clear_action`
5. `paste_event_inserts_text_only_when_composer_is_focused`
6. `bracketed_paste_restoration_disables_bracketed_paste_before_raw_mode`

## GREEN verification
Executed:

```bash
cargo fmt --all && \
cargo test paste_ && \
cargo test composer_key_events && \
cargo test bracketed_paste && \
cargo fmt --all -- --check && \
cargo clippy --workspace --all-targets -- -D warnings
```

Observed results:
- `cargo test paste_`: 5 passed
- `cargo test composer_key_events`: 1 passed
- `cargo test bracketed_paste`: 1 passed
- `cargo fmt --all -- --check`: OK
- `cargo clippy --workspace --all-targets -- -D warnings`: OK

## Notes
- The implementation keeps Task 3 local to TUI boundary code and avoids changing `src/app.rs`.
- Clipboard failures currently surface in the composer title via `ViewState.notification`; this satisfies the brief's concise notification requirement without widening UI scope.
