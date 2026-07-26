# Changelog

## 2026-07-26

### Added

- Added the headless Rust agent core with versioned provider contracts, credential storage,
  streaming events, cancellation, local tool execution, and bounded session orchestration.
- Added the bundled OpenAI provider with ChatGPT subscription OAuth, local model discovery,
  credential refresh, Responses API streaming, encrypted-reasoning replay, and correlated tool
  calls.
- Added interactive provider, authentication, and model-selection flows to the TUI without eagerly
  starting provider processes.
- Added a fullscreen conversation transcript with streamed assistant output, tool activity,
  cancellation feedback, and a responsive startup card that scrolls away with earlier content.
- Added a multiline composer with `Shift+Enter` and `Ctrl+J`, cursor-relative Unicode editing,
  bracketed paste, forwarded `Cmd+V`, and mouse-based cursor positioning.
- Added persistent prompt and slash-command history across Misy processes, retaining the latest
  100 accepted entries.
- Added full-screen mouse selection with OSC 52 copy handoff so terminal hosts such as Herdr retain
  clipboard ownership and copy feedback.
- Added Anthropic Claude Pro/Max and Kimi For Coding subscription providers.
- Upgraded the provider contract to protocol version 2 with browser, device, prompt, and no-auth
  flows, plus separate authentication session and completion payloads.
- Added dynamic model discovery with bundled fallbacks for OpenAI, Anthropic, and Kimi.
- Added the `/usage` command for viewing normalized account limits from the provider of the
  currently selected model.
- Added the `/exit` command for cancelling active work and shutting down Misy cleanly.
- Added `-c <path>` and `--config=<path>` options for selecting the Misy configuration directory.

### Improved

- Made the slash-command popup show up to eight scrollable, cyclically selectable commands, with
  `Tab` completing the selected command in the composer without executing it.
- Made `Ctrl+C` interrupt active work and require a highlighted, time-bounded second press before
  exiting an idle TUI.
- Added core-owned FIFO prompt scheduling, a bounded queued-prompt preview, explicit
  `Thinking…`/`Responding…` activity, and single- or double-`Esc` cancellation for the active turn
  or the whole queue.
- Kept provider and model operations non-blocking so streaming and keyboard input remain
  responsive.
- Hardened provider authentication, stream request correlation, cancellation, credential
  persistence, concurrent session handling, and provider-process cleanup.
- Added project architecture, provider, TUI, development, testing, and code-style documentation.
