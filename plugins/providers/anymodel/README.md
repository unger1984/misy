# AnyModel provider

Standalone Misy provider for the AnyModel OpenAI-compatible API. It uses a masked API-key prompt,
discovers the authenticated upstream catalog, and translates normalized Misy turns to streaming
chat completions. Model IDs remain provider-local: for example, Misy displays
`anymodel/cx/gpt-5.6-sol` and this plugin sends `cx/gpt-5.6-sol` upstream.
Explicit context, description, and pricing facts come from the authenticated catalog. Missing
facts are enriched from AnyModel's official public model pages, whose final route prices differ
from underlying vendor list prices. Successful public metadata is cached for 24 hours and retained
across transient refresh failures; unavailable facts render as blank cells in clients.

The core owns opaque credential storage. This package reads `MISY_ANYMODEL_BASE_URL`,
`MISY_ANYMODEL_REQUEST_TIMEOUT_MS`, and the `MISY_ANYMODEL_PUBLIC_CATALOG_*` cache, URL, and timeout
overrides; it deliberately does not read an API-key environment variable. For models advertising
or supporting normalized reasoning effort, it sends the separately selected thinking level as
`reasoning_effort`; model selectors remain colon-delimited in Misy.
