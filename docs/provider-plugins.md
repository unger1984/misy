# Provider Plugins

## Table of Contents

- [Purpose](#purpose)
- [Package Boundary](#package-boundary)
- [Discovery and Lifecycle](#discovery-and-lifecycle)
- [Protocol Version 2](#protocol-version-2)
- [Optional Capabilities](#optional-capabilities)
- [Responsibilities](#responsibilities)
- [Environment Overrides](#environment-overrides)
- [Change Impact](#change-impact)
- [Sources of Truth](#sources-of-truth)

## Purpose

Read this document before changing provider packages, manifests, discovery, process lifecycle,
authentication, model listing, or streaming.

## Package Boundary

Each provider is a self-contained publishable package under `plugins/providers/<provider-id>/`
when bundled or the matching user plugin directory when installed. It contains `misy-plugin.json`,
documentation, a license, language-native build metadata/lockfiles, source, and tests.

Plugins may use any language that can run as a process and speak the protocol. Bun is the
implementation choice for the bundled `openai`, `anthropic`, and `kimi` packages, not a global
host dependency.

### Shared TypeScript SDK

The TypeScript providers share an optional implementation library at `plugins/providers/_sdk/`
(published nowhere; imported as `@misy/provider-sdk`). It owns the NDJSON transport, the JSON-RPC
envelope and error codes, protocol-method dispatch with strict parameter validation, and helpers
such as `fetchWithTimeout`. Each TypeScript plugin's `src/index.ts` only wires its provider
adapter to `serve(...)` from that library.

The SDK is **not** part of the protocol: plugins in other languages implement the same JSON-RPC
contract independently, and a plugin that outgrows the SDK may do the same. Every plugin remains a
separate process with its own `misy-plugin.json`, spawned exactly as before.

Plugins consume the SDK as a local `file:../_sdk` dependency declared in their `package.json`. Bun
links such a dependency file by file rather than copying it, so every plugin reads the working tree
copy of `_sdk` and an edit there takes effect without reinstalling. The trade-off is that a bundled
plugin directory is not relocatable on its own: the links are absolute paths into this repository,
so moving one elsewhere without `_sdk` breaks its imports. Plugins installed under
`~/.misy/plugins/providers` are unaffected — they declare their own dependencies and never resolve
`file:../_sdk`.

## Discovery and Lifecycle

- Discovery reads and validates manifests without launching plugins.
- Every manifest declares a stable `id`, a user-facing `display_name`, and one or more
  `auth_methods` with stable IDs and user-facing names. Credentials persist the selected method ID.
- Duplicate IDs and incompatible protocol versions are rejected.
- A manifest may declare independently versioned optional capabilities in `capabilities`. Omitted
  `capabilities` means no optional capabilities, preserving compatibility with existing manifests.
- A provider starts lazily when selected or used, remains alive while in use, and is
  terminated/reaped during failure or shutdown.
- Provider stdout is reserved for protocol messages; diagnostics use stderr.

## Protocol Version 2

Transport is JSON-RPC 2.0 with one JSON object per line over stdin/stdout.

Required methods: `auth.status`, `auth.start`, `auth.complete`, `auth.refresh`, `auth.logout`,
`models.list`, `chat.start`, and `chat.cancel`.

`auth.start` returns a discriminated union selected by the required `kind` field:

```jsonc
{ "kind": "browser", "url": "https://…", "session": {…} }
{ "kind": "device", "url": "https://…", "user_code": "WDJB-MJHT",
  "expires_at": 1795000000000, "session": {…} }
{ "kind": "prompt", "fields": [
  { "id": "api_key", "label": "API key", "secret": true }
], "session": {…} }
{ "kind": "none" }
```

Clients reject missing or unknown `kind` values. Browser and device flows require an HTTP(S)
`url`; device flows also require `user_code` and may provide an epoch-millisecond `expires_at`.
The device URL is `verification_uri_complete` when the provider supplies it and otherwise
`verification_uri`. Prompt flows require a non-empty `fields` array, and clients must mask fields
marked `secret`. `session` is an opaque authentication-attempt marker required by every flow
except `none`.

`auth.complete` receives `{ "session": …, "completion": … }`. Browser and device clients send an
empty completion object; prompt clients place entered values in `completion` by field ID.

Adding an optional result field is backward compatible. Adding or renaming a required field,
method, or event requires a protocol version change.

Any successful result may carry an optional top-level `credentials` object with replacement opaque
credentials. A provider includes it when serving the request silently refreshed or rotated tokens
(for example inside `usage.get` or `chat.start`, where refresh-token rotation would otherwise
invalidate the stored credential), and omits it when no refresh occurred. The core persists such
credentials exactly like an `auth.refresh` result and removes the field before the result is
validated or exposed to clients.

Streaming notifications are `text_delta`, `tool_call`, `completed`, and `failed`. Every stream
notification carries the numeric `request_id` of its `chat.start`; cancellation carries
`{ "request_id": ... }`.

`models.list` returns `models` and may include a provider-local `default_model` ID. The core uses
that declared default when it is present and falls back to the first model for older providers.

## Optional Capabilities

The base protocol remains version 2. Optional contracts are negotiated through the manifest rather
than by changing the v2 required-method set. A provider supports a capability only when it declares
the exact version, for example:

```json
{
  "capabilities": {
    "usage": { "version": 1 }
  }
}
```

Unknown capability IDs are permitted for forward compatibility. A missing `capabilities` object,
or a manifest without `usage` version 1, does not implement usage and must not receive `usage.get`.

### Usage capability version 1

When a manifest declares `capabilities.usage.version` as `1`, `usage.get` is required for that
provider. The core sends the selected model snapshot and opaque credentials:

```json
{
  "provider_id": "provider-id",
  "model_id": "provider-model-id",
  "credentials": { "...": "opaque provider credentials" }
}
```

The provider owns the remote endpoint, headers, timeout for its HTTP request, and parsing of the
provider-specific response. It returns a normalized report, never raw upstream payloads:

```json
{
  "fetched_at": 1795000000000,
  "limits": [
    {
      "id": "primary",
      "label": "Primary limit",
      "amount": {
        "used": 42,
        "limit": 100,
        "remaining": 58,
        "unit": "percent"
      },
      "window": { "duration_ms": 10800000, "resets_at": 1795010800000 },
      "status": "ok",
      "notes": []
    }
  ],
  "notes": []
}
```

`fetched_at` is Unix epoch milliseconds. `limits` may be empty. Each limit has a unique,
display-safe `id`, a display-safe `label`, an `amount`, and optional `window`, `status`, and
display-safe `notes`. An amount requires `used` or `remaining`; populated quantities are finite,
non-negative numbers, and a populated `limit` is positive. Supported units are `percent`,
`tokens`, `requests`, `usd`, `minutes`, `bytes`, and `unknown`; statuses are `ok`, `warning`,
`exhausted`, and `unknown`. The core rejects unknown fields and malformed, duplicate, oversized,
or unsafe reports.

Before calling the provider, the core verifies the manifest declaration, uses the snapshotted
selected `ModelRef`, refreshes credentials when needed, injects opaque credentials, and enforces a
30-second JSON-RPC deadline. The core reports unavailable capabilities, authentication failures,
timeouts, transport failures, and invalid normalized reports without exposing credentials or
provider error details.

## Responsibilities

- Rust stores opaque provider credentials, redacts them from public results/errors, and supplies
  them when needed. Remote error messages are additionally capped at the wire boundary and
  stripped of secret-shaped fragments (`Bearer` tokens and `authorization`/`x-api-key` header
  values), while their wording remains the provider's own. On-disk protection is Unix-only: the
  credential file is written atomically with `0600` and `~/.misy` is forced to `0700`; on Windows
  the file inherits the ambient ACL, so restricting access there is left to the user's
  environment.
- A provider implements its own authentication, refresh, account status, model discovery, request
  mapping, stream parsing, and (when declared) usage endpoint/header selection and response
  normalization.
- A provider never reads another client's credential files and never executes Misy's local tools.
- Local tools are advertised and executed by the Rust core; normalized results are returned to the
  provider loop.

## Environment Overrides

Each provider package reads its endpoint and timeout defaults from the process environment before
falling back to production values. This is a supported user feature — for routing through a proxy
or gateway, or for debugging — not a test-only hook. A plugin inherits the environment of the core
process that spawns it, which in turn inherits the user's shell environment; the core does not
inspect or forward these variables. Timeout variables accept positive integers (milliseconds) and
fall back to the default on any other value.

**openai** (`plugins/providers/openai/src/config.ts`)

- `MISY_OPENAI_AUTH_ISSUER` — OAuth issuer base URL.
- `MISY_OPENAI_CLIENT_ID` — OAuth client ID.
- `MISY_OPENAI_BASE_URL` — ChatGPT backend base URL.
- `MISY_OPENAI_OAUTH_SCOPES` — space-separated OAuth scopes.
- `MISY_OPENAI_ORIGINATOR` — originator value sent to the backend.
- `MISY_OPENAI_CLIENT_VERSION` — client version reported to the backend.
- `MISY_OPENAI_AUTH_TIMEOUT_MS` — deadline for the OAuth flow (default 300000).
- `MISY_OPENAI_REQUEST_TIMEOUT_MS` — per-request timeout (default 30000).

**anthropic** (`plugins/providers/anthropic/src/config.ts`)

- `MISY_ANTHROPIC_AUTHORIZE_URL` — OAuth authorization URL.
- `MISY_ANTHROPIC_API_BASE_URL` — API base URL.
- `MISY_ANTHROPIC_CLIENT_ID` — OAuth client ID.
- `MISY_ANTHROPIC_OAUTH_SCOPES` — space-separated OAuth scopes.
- `MISY_ANTHROPIC_AUTH_TIMEOUT_MS` — deadline for the OAuth flow (default 300000).
- `MISY_ANTHROPIC_REQUEST_TIMEOUT_MS` — per-request timeout (default 30000).

**kimi** (`plugins/providers/kimi/src/config.ts`)

- `MISY_KIMI_AUTH_BASE_URL` — OAuth base URL.
- `MISY_KIMI_API_BASE_URL` — API base URL.
- `MISY_KIMI_CLIENT_ID` — OAuth client ID.
- `MISY_KIMI_REQUEST_TIMEOUT_MS` — per-request timeout (default 30000).
- `MISY_KIMI_DATA_DIR` — plugin data directory (default `~/.local/share/misy/kimi`).

## Change Impact

Protocol changes require a version decision plus updates to host, core, provider packages,
fixtures, README, and contract tests. Do not silently widen the current protocol.

## Sources of Truth

- [`crates/misy-core/src/providers/manifest.rs`](../crates/misy-core/src/providers/manifest.rs)
- [`crates/misy-core/src/providers/protocol.rs`](../crates/misy-core/src/providers/protocol.rs)
- [`crates/misy-core/src/providers/host.rs`](../crates/misy-core/src/providers/host.rs)
- [`plugins/providers/openai/README.md`](../plugins/providers/openai/README.md)
- [`plugins/providers/anthropic/README.md`](../plugins/providers/anthropic/README.md)
- [`plugins/providers/kimi/README.md`](../plugins/providers/kimi/README.md)
- [Architecture](architecture.md)
