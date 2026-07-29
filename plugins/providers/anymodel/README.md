# AnyModel provider

Standalone Misy provider for the AnyModel OpenAI-compatible API. It uses a masked API-key prompt,
discovers the authenticated upstream catalog, and translates normalized Misy turns to streaming
chat completions. Model IDs remain provider-local: for example, Misy displays
`anymodel/cx/gpt-5.6-sol` and this plugin sends `cx/gpt-5.6-sol` upstream.

The core owns opaque credential storage. This package reads `MISY_ANYMODEL_BASE_URL` and
`MISY_ANYMODEL_REQUEST_TIMEOUT_MS` for supported endpoint and timeout overrides; it deliberately
does not read an API-key environment variable.
