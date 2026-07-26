# misy

Misy is a fast, minimal agent harness written in Rust. It is designed around a terminal user interface, with planned support for MCP, explicit permissions, and TypeScript plugins powered by Bun.

## Provider packages and protocol

Providers are self-contained packages. Bundled packages belong in
`plugins/providers/<provider-id>/`; user-installed packages belong in
`~/.misy/plugins/providers/<provider-id>/`. Each package contains a
`misy-plugin.json` manifest, its executable/source, and its own documentation,
license, and language-native build metadata. Misy discovers these manifests
without launching their processes; duplicate IDs and protocol versions other
than `1` are rejected.

```json
{
  "id": "example-provider",
  "version": "1.0.0",
  "kind": "provider",
  "protocol_version": 1,
  "description": "Example provider",
  "author": "Example",
  "homepage": "https://example.invalid",
  "repository": "https://example.invalid/repository",
  "license": "MIT",
  "command": "./provider",
  "args": []
}
```

The host starts a selected provider lazily and keeps one process alive while it
is in use. Communication is JSON-RPC 2.0, one JSON object per newline on stdin
and stdout. Version 1 requires `auth.status`, `auth.start`, `auth.complete`,
`auth.refresh`, `auth.logout`, `models.list`, `chat.start`, and `chat.cancel`.
Streaming chat notifications use `text_delta`, `tool_call`, `completed`, or
`failed`. A provider may be implemented in any language; Bun is only the
planned runtime for the bundled Codex provider.

## Motivation

I got tired of fighting with multi-provider agent tools like OpenCode and OMP, so I decided to build my own.

> [!WARNING]
> Misy is in the earliest stages of development. It is not ready for use in any form.
