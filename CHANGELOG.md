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
- Added `-c <path>` and `--config=<path>` options for selecting the Misy configuration directory.

### Improved

- Kept provider and model operations non-blocking so streaming and keyboard input remain
  responsive.
- Hardened provider authentication, stream request correlation, cancellation, credential
  persistence, concurrent session handling, and provider-process cleanup.
- Added project architecture, provider, TUI, development, testing, and code-style documentation.
