# Misy OpenAI provider

This standalone Bun/TypeScript package exposes OpenAI models through a ChatGPT
subscription. It implements Misy provider protocol v2 over JSON-RPC 2.0 NDJSON.
Browser OAuth uses OpenAI's registered `http://localhost:1455/auth/callback`
redirect only; if another login owns that port, it reports a clear error.

The provider never reads or writes Misy or Codex credential files. Misy stores
the opaque credentials returned by `auth.complete`, including `type: "oauth"`.
The `chatgpt-account-id` extracted from the OAuth `id_token` is sent on every
ChatGPT-backed Responses request.

## Installation

Misy discovers bundled packages in `plugins/providers/` and installed packages
in `~/.misy/plugins/providers/`.

```sh
mkdir -p ~/.misy/plugins/providers
cp -R plugins/providers/openai ~/.misy/plugins/providers/
cd ~/.misy/plugins/providers/openai
bun install --production
```

## Models and authentication

`models.list` discovers models available to the authenticated subscription
from the ChatGPT Codex endpoint. It retries the compatibility `/models` path
when the primary route fails, and uses the bundled catalog only when discovery
is unavailable or malformed. The only declared authentication method today is
browser OAuth; the manifest leaves room for a future API-key method without
changing the provider identity.

Pending browser logins expire after five minutes and release the callback port.
Tokens are refreshed when within one minute of expiry and a 401 refreshes then
retries the Responses request exactly once.

Environment variables allow local fake endpoints without credentials:

- `MISY_OPENAI_AUTH_ISSUER`
- `MISY_OPENAI_BASE_URL`
- `MISY_OPENAI_CLIENT_ID`
- `MISY_OPENAI_OAUTH_SCOPES`
- `MISY_OPENAI_ORIGINATOR`
- `MISY_OPENAI_CLIENT_VERSION`
- `MISY_OPENAI_AUTH_TIMEOUT_MS`
- `MISY_OPENAI_REQUEST_TIMEOUT_MS`

## Development

```sh
bun test
bunx tsc --noEmit
bunx biome check .
```

Biome's `useLiteralKeys` rule is disabled because the required TypeScript
`noPropertyAccessFromIndexSignature` option deliberately requires bracket access for untyped JSON
objects at the provider boundary.
