# Kimi provider

Standalone Misy provider for Kimi For Coding subscriptions. It uses Kimi's OAuth device flow,
lists models dynamically, and translates Misy chat requests to Kimi's OpenAI-compatible streaming
endpoint.

The provider is launched by Misy through `misy-plugin.json`; its stdout is exclusively JSON-RPC
NDJSON. The `MISY_KIMI_AUTH_BASE_URL`, `MISY_KIMI_API_BASE_URL`, `MISY_KIMI_CLIENT_ID`,
`MISY_KIMI_DATA_DIR`, and `MISY_KIMI_REQUEST_TIMEOUT_MS` overrides support local development and
offline tests.
