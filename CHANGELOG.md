# Changelog

## 2026-07-30

### Added

- Added optional, best-effort Herdr pane lifecycle reporting for interactive Misy sessions, with
  semantic idle/working/blocked state, coalesced shell-free subprocess calls, and shutdown release.

### Fixed

- Made the model picker present model, context, cost, provider, and description as aligned table
  columns instead of expanding the selected row into a multi-line metadata block. Missing catalog
  values stay blank. AnyModel enriches missing price, context, name, and description fields from
  its official public catalog with a 24-hour stale-preserving cache, and sends selected thinking
  levels through its documented `reasoning_effort` field. It no longer fabricates a 128k context
  window. OpenAI reasoning metadata restores the full verified level set when live discovery
  returns only one level; `Esc` returns to the model list.
- Kept long-list viewports stationary while the cursor moves within their visible rows, scrolling
  only after the cursor reaches the top or bottom edge.
- Added Kimi K3 and K3-256K thinking selection with the official `low`, `high`, and `max` levels,
  defaulting to `high` and sending the choice as `reasoning_effort` through both direct Kimi and
  AnyModel routes.
- Passed the pane identifier before reporting options as required by the Herdr CLI, allowing Misy
  sessions to appear in the Herdr Agents sidebar.

## 2026-07-29

### Added

- Added layered hot-reloaded agent roles, exact model search, provider-owned thinking selection,
  nested canonical agent trees, and visible ordered model attempts.
- Added append-only manual and automatic context compaction with resumable active-history
  checkpoints and separate `/context` accounting.

- Added the core-owned `SetTodoList` tool with full-snapshot updates, root-session persistence,
  resume support, child-agent isolation, and semantic TUI transcript rendering.
- Added the capability-gated `AskUserQuestion` tool with single- and multi-select prompts, custom
  answers, cancellation-safe response handling, dismissal suppression, and an interactive TUI
  dialog.
- Added hierarchical `AGENTS.md` context with mandatory global and project-root instructions,
  target-scoped nested rules, stable main/child caches, leaf-first budgets, and atomic tool-batch
  preflight before filesystem side effects.
- Added a scrollable `/context` popup backed by a content-free core report for model window,
  estimated prompt categories, active instruction sources, decoded image bytes, and warnings.
- Added the bundled AnyModel provider with masked API-key authentication, a dynamic namespaced
  model catalog, OpenAI-compatible streaming, tool calls, and image input.
- Added generic protocol v2 prompt forms to the TUI with secret masking, editing, paste,
  cancellation, and transient credential buffers.
- Added independent synchronous and background child agents with inherited context, correlated
  provider streams, scoped command ownership, addressable messaging, bounded transcripts, and a
  pull-based completion mailbox.
- Added agent roster, preview, fullscreen transcript, stop handling, and completion notices to the
  shared TUI activity workflow.
- Added private, versioned, append-only conversation sessions with automatic incremental
  persistence, cwd-scoped resume, model restoration, transcript replay, `/new`, `/clear`,
  `/resume [id]`, `--continue`, and `--resume [id]`.
- Added clipboard image paste through `Ctrl+V` and `Cmd+V`, including image-only prompts,
  removable composer placeholders, multimodal OpenAI, Anthropic, and Kimi requests, and the
  core-owned `view_image` tool.
- Added optional Unix PTY command sessions with interactive `write_stdin`, bounded polling,
  process-group termination, and a fullscreen TUI activity log viewer.

### Changed

- Expanded provider catalogs with source freshness, descriptions, pricing text, reasoning levels,
  completed-token usage, stale-cache retention, and compact context-window presentation.
- Changed the agent limit to a configurable root-inclusive tree budget and made role/parent tool
  intersections authoritative for nested spawning.

- Moved child role guidance and all `AGENTS.md` contents into an ephemeral request system prefix;
  session JSONL and visible transcripts retain neither instruction contents nor internal scope
  retry turns.
- Redesigned the session transcript with a consistent left inset, distinct user and assistant
  blocks, paired friendly tool calls and results, compact head/tail output, global `Ctrl+O`
  expansion, background-task completion cards, and successful tool-work separators.
- Moved the complete `/provider` workflow into the centered popup used by `/model`, including the
  provider list, provider actions, authentication progress, device codes, and logout progress.
- Unified model-facing command execution under `exec_command`, with inline completion, automatic
  bounded yield, explicit background execution, ordered output, and a 64-process core limit.

### Fixed

- Added a Kimi-style progress header to sticky todos and aligned them and the live generation
  indicator with the transcript content inset.
- Added a separate review-and-submit tab for multi-question requests while preserving immediate
  submission for a single question, and compacted completed interactive-tool headers to `Used`.
- Translated Misy history into strict OpenAI Chat Completions messages for AnyModel instead of
  forwarding internal metadata and tool records that upstream rejects with HTTP 400.
- Made AnyModel rate-limit failures actionable and deterministic instead of racing a terminal
  stream failure against a second JSON-RPC error for the same chat request.
- Treated the Kimi-style question header length as model guidance instead of rejecting the tool
  call, truncated long tabs safely, and hid the generation indicator while awaiting an answer.
- Rendered pending Kimi-style questions inline in the composer slot, preserving hidden drafts and
  attachments while keeping the current todo snapshot visible with status-specific markers.
- Rendered provider prompt authentication as an explicit in-popup input field with a visible
  editing cursor and validation hint, so AnyModel API-key entry is discoverable and actionable.
- Kept active conversations usable after session-storage failures, tolerated malformed trailing
  JSONL records, and rejected missing, ambiguous, incompatible, or active-work resume attempts.
- Prevented foreground commands from being terminated when they transition to background while
  all remaining process permits are occupied.
- Preserved pasted images when recalling the latest submitted prompt with `Up`, while keeping
  persisted prompt history text-only and bounding in-memory image history.
- Aligned OpenAI and Kimi image capability discovery and request routing with their reference
  clients, including OpenAI Responses image detail and Kimi's model-specific wire protocol.
- Made `Esc` cancel a pending provider browser/device authentication wait, restore the provider
  actions immediately, and terminate the blocked provider process so a later attempt starts clean.
- Made provider rows allocate width adaptively and truncate arbitrary long names or statuses with
  an ellipsis, keeping both columns readable and visually separated in narrow popups.

## 2026-07-27

### Changed

- Added `/status` as a provider-neutral alias for `/usage` across the bundled OpenAI, Anthropic,
  and Kimi providers.
- Split the Rust project into the headless `misy-core` library and `misy-tui` client crates in a
  Cargo workspace.
- Migrated core orchestration, provider processes, event subscriptions, tools, and cancellation
  to Tokio while keeping a dedicated core-owned runtime.
- Made `CoreSnapshot` the client contract for selected-model, authentication, and submission-queue
  state.
- Restricted the `misy-core` public surface to the client contract: provider-host, catalog,
  credential-store, model-cache, and tool internals moved behind the `test-support` feature used
  only by integration tests, and `ProviderHost` now always runs on an explicit runtime handle.

### Fixed

- Preserved rapid and duplicate prompts as distinct FIFO entries, kept queued prompts visible
  until their turns start, and prevented assistant output from adjacent turns from merging.
- Made `Esc` and active `Ctrl+C` cancel only the captured active turn without clearing queued
  prompts or emitting duplicate cancellation rows.
- Published terminal submission events only after the completed turn is removed from the core
  snapshot, preventing a stale busy indicator.
- Preserved system keyboard-layout symbols such as `?` and shifted digits while retaining
  `Shift+Enter` multiline input.

## 2026-07-26

### Added

- Added the headless Rust agent core with versioned provider contracts, credential storage,
  streaming events, cancellation, local tool execution, and bounded session orchestration.
- Added the bundled OpenAI provider with ChatGPT subscription OAuth, local model discovery,
  credential refresh, Responses API streaming, encrypted-reasoning replay, and correlated tool
  calls.
- Added interactive provider, authentication, and model-selection flows to the TUI without eagerly
  starting provider processes.
- Added a fullscreen conversation transcript with streamed assistant output, tool activity,
  cancellation feedback, and a responsive startup card that scrolls away with earlier content.
- Added a multiline composer with `Shift+Enter` and `Ctrl+J`, cursor-relative Unicode editing,
  bracketed paste, forwarded `Cmd+V`, and mouse-based cursor positioning.
- Added persistent prompt and slash-command history across Misy processes, retaining the latest
  100 accepted entries.
- Added full-screen mouse selection with OSC 52 copy handoff so terminal hosts such as Herdr retain
  clipboard ownership and copy feedback.
- Added Anthropic Claude Pro/Max and Kimi For Coding subscription providers.
- Upgraded the provider contract to protocol version 2 with browser, device, prompt, and no-auth
  flows, plus separate authentication session and completion payloads.
- Added dynamic model discovery with bundled fallbacks for OpenAI, Anthropic, and Kimi.
- Added the `/usage` command for viewing normalized account limits from the provider of the
  currently selected model.
- Added the `/exit` command for cancelling active work and shutting down Misy cleanly.
- Added `-c <path>` and `--config=<path>` options for selecting the Misy configuration directory.

### Improved

- Made the slash-command popup show up to eight scrollable, cyclically selectable commands, with
  `Tab` completing the selected command in the composer without executing it.
- Made `Ctrl+C` interrupt active work and require a highlighted, time-bounded second press before
  exiting an idle TUI.
- Added core-owned FIFO prompt scheduling, a bounded queued-prompt preview, explicit
  `Thinking…`/`Responding…` activity, and single- or double-`Esc` cancellation for the active turn
  or the whole queue.
- Kept provider and model operations non-blocking so streaming and keyboard input remain
  responsive.
- Hardened provider authentication, stream request correlation, cancellation, credential
  persistence, concurrent session handling, and provider-process cleanup.
- Added project architecture, provider, TUI, development, testing, and code-style documentation.
