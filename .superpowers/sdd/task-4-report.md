# Task 4 — Codex subscription provider report

## Implementation

Added `plugins/providers/codex-subscription/` as a standalone Bun/TypeScript
provider package. It includes the manifest, package metadata and lockfile,
MIT license, installation/development README, TypeScript source, and focused
local-server tests.

The provider implements all protocol-v1 methods:

- `auth.status`, `auth.start`, `auth.complete`, `auth.refresh`, and
  `auth.logout`.
- Browser Authorization Code + PKCE using a one-use in-memory loopback callback
  server. OAuth client, issuer, scopes, and API endpoints are injectable through
  `MISY_CODEX_*` environment variables.
- Dynamic `GET /models` discovery from the ChatGPT Codex base URL.
- `POST /responses` SSE streaming with text deltas, function definitions,
  function calls, previous function-call history, and function-call outputs.
- `chat.cancel` aborts the matching numeric `chat.start` request. EOF aborts
  active chats before the process exits.
- Strict stdout discipline: only JSON-RPC/NDJSON is written to stdout.

OAuth defaults follow the current public Codex reference: issuer
`https://auth.openai.com`, Codex client ID
`app_EMoamEEZ73f0CkXaXp7hrann`, the Codex scopes, and the Codex base URL
`https://chatgpt.com/backend-api/codex`. No code accesses `~/.codex` or invokes
the Codex executable. Credentials remain opaque JSON returned to the Rust
credential store.

Minimal core integration was necessary: the core now adds its already-stored
provider credentials to `chat.start`, just as it does for non-streaming provider
methods. A Rust regression test proves that handoff.

## TDD evidence

RED commands and observed failures:

1. `bun test tests/provider.test.ts` — failed because `../src/provider` did not
   exist; this was the initial desired API test before provider implementation.
2. `bun test tests/runtime.test.ts` — failed with `Module not found
   "src/index.ts"`; the JSON-RPC runtime did not yet exist.
3. `cargo test core_passes_stored_credentials_to_streaming_chat_requests` —
   failed the expected `authenticated` text assertion before credentials were
   added to `chat.start`.
4. `bun test tests/provider.test.ts` — failed because the Responses input
   lacked the expected prior `function_call` record before its result.
5. `bun test tests/provider.test.ts` — failed because the OAuth authorization
   URL lacked the Codex scopes and Codex login parameters.

GREEN commands and results:

- `bun test` in `plugins/providers/codex-subscription` — **5 pass, 0 fail,
  26 assertions** (OAuth, refresh/models/SSE text+functions+results, state
  failure, NDJSON, cancellation, parse/method errors).
- `bunx tsc --noEmit` in that package — passed with no diagnostics.
- `cargo test --test core` — **20 pass, 0 fail**, including the new credential
  handoff regression.
- `cargo clippy --lib -- -D warnings` — passed.
- `rustfmt --edition 2024 --check src/core/agent.rs tests/core.rs` — passed.
- `git diff --check` — passed.

## Files

- `plugins/providers/codex-subscription/{misy-plugin.json,package.json,bun.lock,tsconfig.json,README.md,LICENSE}`
- `plugins/providers/codex-subscription/src/{index.ts,provider.ts}`
- `plugins/providers/codex-subscription/tests/{provider.test.ts,runtime.test.ts}`
- `src/core/agent.rs`
- `tests/core.rs`
- `tests/fixtures/core_provider_fixture.sh`

## Self-review

- Request-correlated stream notifications always use the numeric JSON-RPC
  `chat.start` ID; cancellation uses the required `{request_id}` payload.
- OAuth state is checked before token exchange and callback/session resources
  are cleaned up in `finally`.
- HTTP behavior is dependency-injected via environment configuration and all
  automated provider tests use local loopback servers only.
- The provider only transforms opaque credentials and never creates a
  user-home credential file.

## Concerns

Full workspace `cargo fmt --all -- --check` and `cargo test` could not be used
as final gates because concurrent, unrelated TUI work has an existing formatting
diff in `tests/tui.rs` and currently imports private `tui::render`. Those files
were not touched or altered. The Task 4-specific Rust test target and library
lint are green.

## Architecture review remediation

The follow-up fixes all three Important findings from the Task 4 architecture
review:

- Refresh responses are merged over the prior opaque credential object. An
  omitted `refresh_token` preserves the previous refresh token, while a supplied
  replacement still rotates it.
- The SSE parser accepts LF, CRLF, and CR event/line separators, including
  multiple `data:` lines.
- A callback with the wrong OAuth state returns HTTP 400 without resolving or
  poisoning the pending login. The same loopback server/session can then accept
  the valid browser callback.

### Remediation TDD evidence

RED — `bun test tests/provider.test.ts`:

- `preserves opaque credentials and the previous refresh token when refresh
  omits one` failed because `refresh_token` and `account_id` were absent.
- `parses CRLF Responses events for text, tool calls, and completion` failed
  because only the fallback `completed` notification was emitted.
- `keeps OAuth pending after a mismatched state and accepts a later valid
  callback` failed because `completeAuth` rejected after the valid callback.
- Result: **3 pass, 3 fail, 23 assertions**.

GREEN — `bun test tests/provider.test.ts` after the minimal fixes:
**6 pass, 0 fail, 23 assertions**.

Final remediation verification:

- `bun test` — **8 pass, 0 fail, 29 assertions**.
- `bunx tsc --noEmit` — passed with no diagnostics.
- `git diff --check` — passed.
- No Rust files changed in this remediation wave, so the previously green
  Rust credential-handoff regression was not rerun.
