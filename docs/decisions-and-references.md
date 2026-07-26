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

## Deferred Scope

MCP, permissions, usage reporting, subagents, marketplace installation/update, persisted
conversations, daemon/public IPC, desktop UI, API-key authentication, generic prompt-based
authentication, and a cross-process credential transaction policy are outside this MVP. The Kimi
subscription device flow is supported through the version 2 provider protocol.

## Reference Policy

`.references/openai-codex/` and `.references/oh-my-pi/` are equal local reference implementations. They are idea and implementation sources, not architectural authorities over Misy.

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
