# Provider Plugins

## Table of Contents

- [Purpose](#purpose)
- [Package Boundary](#package-boundary)
- [Discovery and Lifecycle](#discovery-and-lifecycle)
- [Protocol Version 1](#protocol-version-1)
- [Responsibilities](#responsibilities)
- [Change Impact](#change-impact)
- [Sources of Truth](#sources-of-truth)

## Purpose

Read this document before changing provider packages, manifests, discovery, process lifecycle, authentication, model listing, or streaming.

## Package Boundary

Each provider is a self-contained publishable package under `plugins/providers/<provider-id>/` when bundled or the matching user plugin directory when installed. It contains `misy-plugin.json`, documentation, a license, language-native build metadata/lockfiles, source, and tests.

Plugins may use any language that can run as a process and speak the protocol. Bun is the
implementation choice for `openai`, not a global host dependency.

## Discovery and Lifecycle

- Discovery reads and validates manifests without launching plugins.
- Every manifest declares a stable `id`, a user-facing `display_name`, and one or more
  `auth_methods` with stable IDs and user-facing names. Credentials persist the selected method ID.
- Duplicate IDs and incompatible protocol versions are rejected.
- A provider starts lazily when selected or used, remains alive while in use, and is terminated/reaped during failure or shutdown.
- Provider stdout is reserved for protocol messages; diagnostics use stderr.

## Protocol Version 1

Transport is JSON-RPC 2.0 with one JSON object per line over stdin/stdout.

Required methods: `auth.status`, `auth.start`, `auth.complete`, `auth.refresh`, `auth.logout`, `models.list`, `chat.start`, and `chat.cancel`.

Streaming notifications are `text_delta`, `tool_call`, `completed`, and `failed`. Every stream notification carries the numeric `request_id` of its `chat.start`; cancellation carries `{ "request_id": ... }`.

`models.list` returns `models` and may include a provider-local `default_model` ID. The core uses
that declared default when it is present and falls back to the first model for older providers.

## Responsibilities

- Rust stores opaque provider credentials, redacts them from public results/errors, and supplies them when needed.
- A provider implements its own authentication, refresh, account status, model discovery, request mapping, and stream parsing.
- A provider never reads another client's credential files and never executes Misy's local tools.
- Local tools are advertised and executed by the Rust core; normalized results are returned to the provider loop.

## Change Impact

Protocol changes require a version decision plus updates to host, core, provider packages, fixtures, README, and contract tests. Do not silently widen protocol version 1.

## Sources of Truth

- [`src/providers/manifest.rs`](../src/providers/manifest.rs)
- [`src/providers/protocol.rs`](../src/providers/protocol.rs)
- [`src/providers/host.rs`](../src/providers/host.rs)
- [`plugins/providers/openai/README.md`](../plugins/providers/openai/README.md)
- [Architecture](architecture.md)
