# Decisions and References

## Table of Contents

- [Purpose](#purpose)
- [Fixed Decisions](#fixed-decisions)
- [Deferred Scope](#deferred-scope)
- [Reference Policy](#reference-policy)
- [Change Impact](#change-impact)
- [Sources of Truth](#sources-of-truth)

## Purpose

Read this document before making a new architecture or user-interaction choice.

## Fixed Decisions

- Misy uses a headless Rust core with frontend clients.
- Providers are language-independent standalone subprocess packages over versioned JSON-RPC NDJSON.
- Rust owns credentials and local tools; providers own remote protocol adaptation.
- Every inference explicitly identifies provider and model.
- The TUI is the first client, not the owner of agent orchestration.
- Clipboard image paste uses visible composer placeholders while the core owns normalized image
  bytes and capability validation. `view_image` returns the same rich image attachment contract;
  unsupported selected models fail explicitly instead of switching models.
- Models launch local commands through one `exec_command` shell-string contract. Fast commands
  return inline; explicit background commands publish immediately, and other long commands publish
  the same process after the bounded yield. `timeout_seconds = 0` means no hard deadline.
- Misy follows the Codex pull model for command sessions: no model notification is injected on
  completion, and empty `write_stdin` calls retrieve incremental output and the final `exit_code`.
  `ActivityChanged` and `ActivityFinished` remain client events used by the TUI.
- `tty` is optional and defaults to false. Pipe sessions have closed stdin but remain pollable.
  PTY input and bounded process-group `TERM` to `KILL` cleanup are supported on macOS and Linux;
  Windows ConPTY and Job Object support is deferred and never silently falls back to pipes.
- The command contract deliberately differs from Codex by using `cwd` and `timeout_seconds`, and
  by adding `description` and `run_in_background`. Its initial and incremental output projections
  share the Codex-style 10,000-token default without claiming wire compatibility.
- The shared activity UI follows the local Codex and Oh My Pi session/task picker patterns while
  retaining Misy's core/client boundary. `spawn_agent` supports synchronous and detached runs;
  background results use an explicit pull mailbox. V1 permits four live children, forbids nested
  spawn, and keeps each child history ephemeral and independent.
- From Codex, Misy adopts forked child sessions, request-correlated streams, and addressable
  wait/message/stop operations. From Oh My Pi, it adopts a shared sync/background lifecycle,
  core-owned activity projection, bounded concurrency, and owner-scoped cleanup.
- Conversations are always persisted incrementally in flat, versioned, append-only JSONL files
  under `~/.misy/sessions`. Headers carry canonical cwd and model identity; the resume picker and
  `--continue` are scoped to an exact canonical cwd match. Resume is explicit, appends to the same
  file, tolerates malformed trailing records, and fails on a missing, ambiguous, or unsupported
  session instead of silently creating a new one. `/clear` and `/new` share one handler.
- `SetTodoList` and `AskUserQuestion` follow Kimi's model-visible names and payload shapes.
  Checklists replace a complete ordered snapshot with `pending`, `in_progress`, and `done`
  statuses. Questions accept one to four tabs, two to four choices, single or multiple selection,
  and a client-synthesized `Other` choice. Codex contributes only typed request identity and
  snapshot recovery. In the TUI, a pending question replaces the composer bottom slot while the
  current root todo snapshot stays pinned directly above it; questions are not modal popups. The
  core owns both tools; question capability version 1 is immutable for the core lifetime, and three
  dismissals suppress further dialogs for the same turn owner.
- Misy always reads the optional global `~/.misy/AGENTS.md` and project-root `AGENTS.md` for a new
  root conversation. Nested `AGENTS.md` files are discovered only on the ancestor chain of an
  actual filesystem target, cached per main/child session, and applied before tool side effects.
  Deeper files win a shared leaf-first 32 KiB project budget. Conditional routing prose remains a
  model instruction; the core does not parse natural-language conditions or eagerly load linked
  documents.
- Instruction discovery follows Codex's stable session ownership, Kimi's leaf-first priority, and
  Oh My Pi's separate context diagnostics. Misy deliberately adds stricter blocked-source and
  whole-batch retry semantics. Arbitrary shell text is not parsed: `exec_command` is scoped only by
  its normalized `cwd`, while `write_stdin` inherits the original command activity cwd.
- Kimi's push notification delivery and Oh My Pi's push-oriented task presentation were considered
  but rejected for model delivery. Oh My Pi remains the implementation reference for descendant
  post-order traversal, process-group signalling, and graceful-to-hard tree termination; Misy v1
  scopes its portable PTY guarantee to processes that stay in the created Unix process group.
- Protocol v2 prompt authentication is rendered as a generic TUI form. Secret values are masked
  and remain transient; the Rust core persists only the provider-owned opaque credential object.

## Deferred Scope

MCP, permissions, recursive agents, agent roles/batches/worktree isolation, marketplace
installation/update, daemon/public IPC, desktop UI, and a cross-process credential transaction
policy are outside this MVP. The Kimi
subscription device flow is supported through the version 2 provider protocol.

## Accepted Dependency Risks

- `ratatui` is used with `unstable-rendered-line-info` for `Paragraph::line_count(width)` in
  `crates/misy-tui/src/tui/render.rs` (transcript height for scrolling). The feature may change
  or vanish in any minor ratatui release. When upgrading ratatui, check that transcript scrolling
  still renders all rows; if the API moved, adapt the single call site or pin the last working
  version.

## Reference Policy

`.references/openai-codex/`, `.references/kimi-cli/`, and `.references/oh-my-pi/` are the three
local reference implementations. They are idea and implementation sources, not architectural
authorities over Misy. Kimi CLI is also the primary reference for Kimi provider wire formats.

When a comparable implementation or UX question appears:

1. Inspect all three references for the comparable behavior.
2. Reuse an established pattern that fits Misy's fixed boundaries.
3. Do not silently invent a third alternative when a suitable pattern exists.
4. If the references differ materially and the choice changes architecture or user-visible behavior, present the alternatives to the user unless the behavior is already specified.

## Change Impact

An explicit user decision is required to change fixed architecture. Update [Architecture](architecture.md), the affected domain document, and concise routing in `AGENTS.md` together.

## Sources of Truth

- [Architecture](architecture.md)
- [Provider Plugins](provider-plugins.md)
- [TUI Client](tui-client.md)
- [`AGENTS.md`](../AGENTS.md)
