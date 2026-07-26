# Misy Codex subscription provider

This is a standalone Bun/TypeScript provider package for a ChatGPT Codex
subscription. It implements Misy provider protocol v1 over JSON-RPC 2.0 NDJSON.
It uses an OAuth Authorization Code flow with PKCE and an ephemeral
`127.0.0.1` callback; it never reads, writes, or reuses `~/.codex`, and it does
not invoke the Codex CLI.

## Install locally

Misy discovers bundled packages from `plugins/providers/` and installed
packages from `~/.misy/plugins/providers/`. To install this package for one
user, copy the entire directory (including `misy-plugin.json`) there:

```sh
mkdir -p ~/.misy/plugins/providers
cp -R plugins/providers/codex-subscription ~/.misy/plugins/providers/
cd ~/.misy/plugins/providers/codex-subscription
bun install --production
```

`bun` must be available in `PATH`, because it is the manifest command.

## Authentication and configuration

Call `auth.start`, open the returned URL in a browser, then call
`auth.complete` with the returned opaque `session` object as `completion`.
Tokens are returned only as opaque JSON in the `credentials` result; Misy owns
credential persistence. `auth.refresh`, `auth.status`, and `auth.logout` use
that payload and do not create local credential files.

The production defaults are `https://auth.openai.com` for OAuth and
`https://chatgpt.com/backend-api/codex` for models and Responses. The following
environment variables intentionally make every endpoint testable without real
network access:

- `MISY_CODEX_AUTH_ISSUER`
- `MISY_CODEX_BASE_URL`
- `MISY_CODEX_CLIENT_ID`
- `MISY_CODEX_OAUTH_SCOPES`

The provider accepts current model lists from `GET /models` and streams
`POST /responses` with `Accept: text/event-stream`. Tool definitions become
Responses functions; function calls and later function-call outputs are mapped
to Misy tool calls/results.

## Development

```sh
bun install
bun test
bunx tsc --noEmit
```

Tests start only local fake HTTP servers for OAuth, models, and Responses.
Protocol messages are the sole stdout output; diagnostics must remain on stderr.
