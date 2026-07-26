# Changelog

## 2026-07-26

- Added Anthropic Claude Pro/Max and Kimi For Coding subscription providers.
- Upgraded the provider contract to protocol version 2 with browser, device, prompt, and no-auth
  flows, plus separate authentication session and completion payloads.
- Added dynamic model discovery with bundled fallbacks for OpenAI, Anthropic, and Kimi.
- Added `-c <path>` and `--config=<path>` options for selecting the Misy configuration directory.
