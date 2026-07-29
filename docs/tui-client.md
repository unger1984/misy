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

The `misy-tui` crate is a thin in-process client over `misy_core::MisyCore`. Its binary starts on
Tokio's current runtime; its terminal loop remains a synchronous frame loop (`draw`, 50 ms poll,
then event pump) so animation and input retain predictable cadence. Long core operations run in
Tokio tasks and return through the TUI operation-result channel.

UI state is explicit, rendering is deterministic, and keyboard/input is mapped to actions before
side effects. `UiState` is a client projection: it refreshes core-owned selected-model,
submission-queue, and provider-authentication state from `CoreSnapshot` at startup and after
relevant core events. The transcript remains a TUI-owned projection of the event stream. Provider,
model, authentication, session, and tool orchestration remain in the core.

## Interaction Model

- The interface uses Ratatui's fullscreen alternate screen. The complete conversation remains in
  application-owned render state while Misy is open, with the newest transcript rows above the
  composer. Leaving Misy restores the terminal screen and scrollback that existed before startup.
- The fullscreen layout is ordered transcript, optional one-line busy indicator,
  persistent bordered composer, an optional slash-command popup, an activity row when active or
  recent work exists, and a footer. Provider, model, and activity workflows use centered modal
  popups.
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
  access and display its normal copy feedback. Bracketed paste inserts text atomically at the
  cursor and never submits embedded newlines. Forwarded `Ctrl+V` and `Cmd+V` first inspect the
  clipboard for an image and insert a numbered `[Image #N]` placeholder when one is available;
  otherwise they paste text. Deleting a placeholder removes its payload, and image-only prompts
  are valid. A draft accepts at most four images. `Up` restores the latest submitted image prompt
  with its attachment during the current process; older image entries become text-only to bound
  memory. Persisted input history excludes both attachment
  payloads and their placeholders. Composer placeholders are presentation-only and are not sent
  as prompt text; accepted events carry only the image count needed to reconstruct transcript rows.
- Typing `/` at the beginning of an empty draft opens a filtered command popup below the composer
  without taking focus from it. The popup shows at most eight commands; `Up` and `Down` scroll its
  window and wrap between the first and last matching commands. `Tab` completes the selected
  command with its canonical name and a trailing space, then hides the popup without executing it.
  `Enter` executes the selected or completed command; `Esc` dismisses the popup without changing
  the draft.
- `/status` fetches the selected model provider's account limits in the background and appends a
  normalized, display-safe usage report to the transcript. `/usage` is a compatibility alias; both
  commands take no arguments. The report shows provider-wide and per-limit notes, consumed and
  remaining amounts, exhausted limits, and known reset timing. If no model is selected, the
  provider does not declare usage capability version 1, authentication fails, the request times
  out, or the report is invalid, the TUI renders the error without changing the selected model.
- `/context` opens a dedicated centered popup without provider or filesystem I/O. It renders the
  core-owned report for the selected model and advertised window, estimated Misy prompt,
  `AGENTS.md`, tool-schema, message, and free-space categories, active or last instruction source
  summaries, decoded image count/bytes, and blocked/truncated-source warnings. A zero model window
  is shown as unknown, and images explicitly have no fabricated token estimate. `Up`, `Down`,
  `PageUp`, `PageDown`, `Home`, and `End` scroll the wrapped report; `Esc` closes it.
- `/exit` takes no arguments and exits through the same cancellation, provider shutdown, and
  terminal-restoration path as `Ctrl+C`.
- `/new` and `/clear` start the same clean conversation while retaining the previous persisted
  session. `/resume` opens a filterable, newest-first picker for sessions whose canonical cwd
  exactly matches the current directory; `/resume <id>` accepts an exact id or unambiguous prefix.
  The picker shows the first user message, relative modification time, and a short id. Resume
  rebuilds both the core's provider history and the visible user/assistant/tool transcript, then
  continues appending to the same session file. These operations are rejected while a submission
  is active or queued. The footer shows the attached short session id once persistence begins.
- CLI startup is fresh by default. `--continue` resumes the newest session for the current cwd;
  `--resume` opens the picker; and `--resume <id>` resumes a specific session. The options are
  mutually exclusive. If a saved model is no longer available, Misy keeps the current selection
  and renders a warning instead of failing the resume.
- `/tasks` opens the shared activity popup. The row below the composer remains visible while active
  or recent activities exist and separately counts running tasks, terminal tasks, and active
  agents. `Down` from an empty composer focuses it and `Enter` opens it. The popup has `All`,
  `Agents`, and `Tasks` tabs, includes `Main`, supports filtering, and shows a bounded tail preview
  for the selected task or semantic child-agent transcript. Agent rows show `agent-N`, model,
  title, and status. `Enter` opens command output or the agent transcript in a fullscreen viewer.
  `Up`, `Down`,
  `PageUp`, and `PageDown` scroll; live output follows the tail until the user scrolls upward, and
  `Escape` restores the same popup tab, filter, and selection. `Ctrl+X` immediately stops the
  selected running task or agent through the generic core stop operation. This stop shortcut is named
  `activities.stop` in config version 2 and may be rebound; popup hints use the effective binding.

  ```toml
  version = 2

  [keybindings]
  "activities.stop" = ["Ctrl+X"]
  "transcript.expand" = ["Ctrl+O"]
  ```
- `/provider` opens a centered provider popup. Selecting a provider replaces the popup contents
  with its available `Authorize` or `Log out` actions; authorization progress, device codes, and
  logout progress remain in the same popup. `/model` immediately opens a centered model popup from
  the core-owned catalog cache while refreshing the catalog in the
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
- A pending `AskUserQuestion` request opens a dedicated centered dialog without changing the
  composer draft or attachments. Requests recover from `CoreSnapshot` and appear FIFO. Tabs retain
  selection and custom text; arrows and digits select rows, `Left`/`Right`/`Tab` switch questions,
  `Space` toggles multi-select choices, and `Enter` advances or submits once every question has an
  answer. The synthetic `Other` row owns an inline editor. `Esc` leaves that editor first and
  otherwise dismisses only the current request through the typed core operation. Completed
  `SetTodoList` and `AskUserQuestion` calls render their semantic lists and answers instead of raw
  JSON, including after session replay.
- Transcript rows use a consistent two-column left inset. Submitted prompts occupy a contrasting
  full-width row inside that transcript area; assistant segments have one leading marker, service
  messages remain dim, and failures have a red marker. Tool calls use friendly built-in names with
  an indented result attached by call ID;
  pending, successful, and failed calls have distinct markers. Results keep their head and tail in
  a width-aware four-row compact budget. The named `transcript.expand` shortcut (`Ctrl+O` by
  default) globally toggles a twelve-row budget without changing the draft or active popup; an
  effective-binding hint appears only when compact output is hidden. Successful turns that used a
  tool end with a separator, including `Worked for Xm Ys` only after one minute. An active
  submission adds an animated one-line spinner with elapsed time and the `esc to interrupt` hint
  above the composer.
- Entered prompts join a bounded queue preview above the composer once the core accepts them:
  the core emits a self-sufficient `SubmissionAccepted` event — carrying the submission ID and
  the message — under the queue lock before enqueueing, so acceptances arrive in FIFO order and
  before any other event for the same submission. The client maps each prompt by its submission
  ID from that event alone and never reconstructs the prompt or the submission order from the
  asynchronous submit result. Accepted prompts enter the transcript when the core starts them.
  Thus repeated identical prompts remain distinct, FIFO transcript entries and no assistant
  output can be merged or misplaced by event/result interleaving. Before the first assistant
  text the activity row says `Thinking…`; once text begins it says `Responding…`.

## Lifecycle and Safety

- The TUI keeps the current draft and pasted images until the core accepts a submission. If image
  normalization or the selected provider/model capability check fails, the draft remains editable
  and the error is shown. The core repeats validation at its public submission boundary.

- Opening the provider list does not start every plugin; local manifest/credential state is used
  until a concrete provider action requires the process.
- Provider authentication follows the `auth.start` kind. `browser` validates and opens the URL,
  then waits for provider completion. `device` does the same while showing the user code in the
  provider operation row. `none` immediately marks the provider authenticated without opening a
  browser. `prompt` reports that field input is not supported by this client yet.
- `Esc` cancels an in-flight provider authentication start or browser/device completion wait,
  restores the provider actions in the popup, and asks the core to terminate the blocked provider
  process. The next attempt starts a clean provider process; late results from the cancelled task
  cannot change the popup or authentication state.
- Browser and device authorization URLs are limited to validated HTTP(S) addresses and are passed
  directly to the OS opener without a shell. Unknown authentication kinds are reported as errors.
- Authentication completion keeps the opaque provider session separate from the empty browser or
  device completion object and passes both through the core without pasted credential JSON.
- During active work, `Ctrl+C` interrupts the current operation without exiting. While idle, the
  first press highlights `press Ctrl+C again to exit` in the footer for one second; a second press
  inside that window shuts down the core/provider host, restores the terminal, and exits. Any
  other input clears the armed shortcut. `/exit` performs the same clean shutdown immediately.
- Event and background-operation processing are bounded per tick so continuous streaming cannot
  starve input handling.
- A background `AgentFinished` event adds one compact transcript notice. The model-facing mailbox
  remains independent, and a missed lossy event can be recovered from `CoreSnapshot.agents` and
  `agent_transcript`. Synchronous agent results are already visible as tool results and are not
  duplicated as notices.
- Conversation persistence belongs to the core. The TUI only requests list/new/resume operations
  and projects the returned canonical history; it never reads or writes session JSONL directly.
- When session switching encounters retained agent state, the TUI preserves it until the user
  confirms discard; confirmation calls the core cleanup operation before retrying the switch.
- The TUI never owns task processes. It polls bounded client output while an activity preview or
  log viewer is visible and sends stop requests through the core; core shutdown remains responsible
  for process-group cleanup. Ordered fragments preserve the stdout/stderr order observed by the
  core capture tasks, with stderr labelled in place.
- A background command's terminal core event adds one transcript item with its task ID, label,
  final output, and exit status. It uses the same compact/expanded transcript budgets as ordinary
  tool results, while the activity log viewer retains the complete bounded output. The initial tool
  result is rendered as a compact background-start notice instead of raw command-result JSON.
- `Esc` interrupts the active turn identified by the current core snapshot. Repeated presses are
  idempotent for that turn and never clear the remaining FIFO queue; the next queued prompt starts
  after cancellation.
- Usage is a core operation, not a TUI-owned provider request: the TUI supplies no endpoint,
  headers, credentials, or provider-specific parsing.

## Change Impact

User-visible interaction changes require comparison with both local references and focused
behavior tests. Core events or lifecycle changes also require updates to
[Architecture](architecture.md).

## Sources of Truth

- [`crates/misy-tui/src/lib.rs`](../crates/misy-tui/src/lib.rs)
- [`crates/misy-tui/src/tui/client.rs`](../crates/misy-tui/src/tui/client.rs)
- [`crates/misy-tui/src/tui/state.rs`](../crates/misy-tui/src/tui/state.rs)
- [`crates/misy-tui/src/tui/context_popup.rs`](../crates/misy-tui/src/tui/context_popup.rs)
- [`crates/misy-tui/src/tui/terminal.rs`](../crates/misy-tui/src/tui/terminal.rs)
- [`crates/misy-tui/tests/tui.rs`](../crates/misy-tui/tests/tui.rs)
- [`crates/misy-tui/tests/tui_model_popup.rs`](../crates/misy-tui/tests/tui_model_popup.rs)
- [`crates/misy-core/src/core.rs`](../crates/misy-core/src/core.rs)
- [Decisions and References](decisions-and-references.md)
