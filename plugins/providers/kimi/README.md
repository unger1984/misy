# Kimi provider

Standalone Misy provider for Kimi For Coding subscriptions. It uses Kimi's OAuth device flow,
lists models dynamically, and translates Misy chat requests to Kimi's OpenAI-compatible streaming
endpoint.

The manifest declares optional `usage` capability version 1. Its required `usage.get` method
uses Kimi's usage endpoint with provider-specific subscription headers and normalizes returned
quota buckets into Misy's usage report. The core injects opaque credentials and validates that
report; this package owns the endpoint, headers, and upstream response parsing.

The provider is launched by Misy through `misy-plugin.json`; its stdout is exclusively JSON-RPC
NDJSON. The `MISY_KIMI_AUTH_BASE_URL`, `MISY_KIMI_API_BASE_URL`, `MISY_KIMI_CLIENT_ID`,
`MISY_KIMI_DATA_DIR`, and `MISY_KIMI_REQUEST_TIMEOUT_MS` overrides support local development and
offline tests.
