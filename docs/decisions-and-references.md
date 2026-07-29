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
  retaining Misy's core/client boundary. Agent sessions remain deferred, but their activity kind,
  tabs, and `Main` navigation slot are reserved by the contract.
- Conversations are always persisted incrementally in flat, versioned, append-only JSONL files
  under `~/.misy/sessions`. Headers carry canonical cwd and model identity; the resume picker and
  `--continue` are scoped to an exact canonical cwd match. Resume is explicit, appends to the same
  file, tolerates malformed trailing records, and fails on a missing, ambiguous, or unsupported
  session instead of silently creating a new one. `/clear` and `/new` share one handler.
- Kimi's push notification delivery and Oh My Pi's push-oriented task presentation were considered
  but rejected for model delivery. Oh My Pi remains the implementation reference for descendant
  post-order traversal, process-group signalling, and graceful-to-hard tree termination; Misy v1
  scopes its portable PTY guarantee to processes that stay in the created Unix process group.

## Deferred Scope

MCP, permissions, subagents, marketplace installation/update, daemon/public IPC, desktop UI,
API-key authentication, generic prompt-based
authentication, and a cross-process credential transaction policy are outside this MVP. The Kimi
subscription device flow is supported through the version 2 provider protocol.

## Accepted Dependency Risks

- `ratatui` is used with `unstable-rendered-line-info` for `Paragraph::line_count(width)` in
  `crates/misy-tui/src/tui/render.rs` (transcript height for scrolling). The feature may change
  or vanish in any minor ratatui release. When upgrading ratatui, check that transcript scrolling
  still renders all rows; if the API moved, adapt the single call site or pin the last working
  version.

## Reference Policy

`.references/openai-codex/` and `.references/oh-my-pi/` are equal local reference implementations. They are idea and implementation sources, not architectural authorities over Misy. `.references/kimi-cli/` is the upstream Kimi CLI, consulted for Kimi provider wire formats and behavior.

When a comparable implementation or UX question appears:

1. Inspect both references.
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
