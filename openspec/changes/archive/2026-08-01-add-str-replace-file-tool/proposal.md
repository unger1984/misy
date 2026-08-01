## Why

Misy can read files and rewrite them wholesale, but it cannot safely make a small, targeted edit.
Adding an exact string-replacement tool removes the need for fragile shell commands or complete
file rewrites during ordinary coding-agent work.

## What Changes

- Add the model-visible `StrReplaceFile` tool for replacing an exact UTF-8 fragment in an existing
  regular file.
- Support one unambiguous replacement by default and explicit replacement of every occurrence.
- Reject missing, empty, unchanged, ambiguous, oversized-input, or oversized-result edits without
  modifying the target file.
- Apply the existing path-scoped hierarchical `AGENTS.md` preflight before the tool can write.
- Bound the complete input and projected result to 4 MiB and validate the projected byte length
  before allocating replacement output.
- Cover the tool definition, validation, boundary behavior, execution, failure behavior, and
  instruction scoping with focused Rust tests.

## Capabilities

### New Capabilities

- `str-replace-file`: Exact, validated string replacement in an existing local file through a
  core-owned agent tool.

### Modified Capabilities

None.

## Impact

- `misy-core` gains one provider-visible tool definition, filesystem operation, dispatcher route,
  and path-scope policy entry.
- Provider plugins and the provider protocol do not change because the tool uses the existing
  JSON Schema function-tool contract and remains executed by the Rust core.
- The TUI continues to render the call through its generic tool presentation; no new orchestration
  responsibility moves into the frontend.
- Tool documentation and focused core integration tests are updated. No new dependency is needed.
