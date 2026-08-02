## Why

The built-in `read_file` tool can only return a file from the beginning, so agents must use
`exec_command` with utilities such as `sed` or `tail` to inspect a focused line range. Optional
pagination will make source inspection more precise and reduce unnecessary model context while
keeping the existing simple call compatible.

## What Changes

- Add optional one-based line `offset` and positive line `limit` arguments to `read_file`.
- Return a bounded contiguous line range when either pagination argument is supplied.
- Include concise range and continuation metadata so an agent can request the next page without
  guessing.
- Preserve the existing whole-file behavior and safety checks when pagination is omitted.
- Keep hierarchical `AGENTS.md` scope preflight based on the requested file path.

## Capabilities

### New Capabilities

- `paged-file-read`: Defines compatible, bounded line-range reads through the core-owned
  `read_file` tool.

### Modified Capabilities

None.

## Impact

- Affects the provider-visible `read_file` JSON Schema and description in `misy-core`.
- Affects bounded UTF-8 file reading and tool-result formatting in `misy-core`.
- Requires focused registry, dispatcher, and core preflight regression coverage.
- Does not change provider protocol versions, frontend/core ownership, or add dependencies.
