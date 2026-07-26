# Misy Anthropic provider

This standalone Bun/TypeScript package exposes Anthropic models through a Claude Pro or Max
subscription. It implements Misy provider protocol v2 over JSON-RPC 2.0 NDJSON.

OAuth uses the `claude.ai` authorization endpoint. `platform.claude.com` is intentionally not an
alternative: it issues console tokens that do not carry the `user:inference` permission required
for subscription inference. The fixed local callback at `http://localhost:54545/callback` expires
after five minutes and reports a useful error when another authorization already owns the port.

Misy owns stored credentials. This plugin never reads or writes Misy credential files. Every
network request has a deadline; provider diagnostics never use stdout, which remains reserved for
JSON-RPC messages.

## Development

```sh
bunx tsc --noEmit
bun test
bunx biome check .
```

The tests use only local fake endpoints. `MISY_ANTHROPIC_AUTHORIZE_URL`,
`MISY_ANTHROPIC_API_BASE_URL`, `MISY_ANTHROPIC_AUTH_TIMEOUT_MS`, and
`MISY_ANTHROPIC_REQUEST_TIMEOUT_MS` exist only to support that isolated testing.
