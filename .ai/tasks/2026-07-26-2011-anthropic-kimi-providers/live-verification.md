# Live Provider Verification

Date: 2026-07-26

Automated provider checks use local fake endpoints and pass without real credentials. The live
subscription checks below were not run because they require the user's OpenAI, Anthropic, and Kimi
accounts plus interactive browser approval. No existing credential files were inspected.

- [ ] Authenticate all three providers through the TUI.
- [ ] Confirm the Kimi device code matches the browser prompt.
- [ ] Load a non-empty live model catalog from every provider.
- [ ] Stream one short live request from every provider.
- [ ] Cancel each provider during a live stream.
- [ ] Log out and confirm each provider returns to `not authenticated`.
