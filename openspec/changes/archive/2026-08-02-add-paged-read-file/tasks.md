## 1. Tool Contract

- [x] 1.1 Extend the provider-visible `read_file` definition with optional one-based `offset` and
  bounded `limit` arguments, updated model guidance, and the existing `path` scope policy.
- [x] 1.2 Add registry tests proving `path`-only compatibility, accepted partial pagination, the
  1000-line maximum, and rejection of zero, negative, non-integer, oversized, and unknown values.

## 2. Bounded Page Projection

- [x] 2.1 Refactor bounded UTF-8 loading to retain readable content plus the physical-file
  truncation state while preserving the exact unpaged result and existing target errors.
- [x] 2.2 Implement line-span selection for paged mode without normalizing LF, CRLF, empty-file, or
  unterminated-final-line content.
- [x] 2.3 Format stable paged metadata for returned ranges, empty pages, `next_offset`, physical EOF,
  and the 4 MiB read boundary, then route optional pagination arguments through the dispatcher.

## 3. Behavioral Coverage and Documentation

- [x] 3.1 Add focused dispatcher tests for defaulted arguments, middle and final pages, offsets after
  EOF, exact next offsets, and boundary-truncated files while retaining existing whole-file tests.
- [x] 3.2 Add a core integration regression proving paged reads still activate a newly applicable
  hierarchical `AGENTS.md` scope before filesystem access.
- [x] 3.3 Update the English and Russian user documentation and changelog to describe paged
  `read_file` usage and its readable-prefix boundary.

## 4. Verification

- [x] 4.1 Run Rust formatting and the focused `misy-core` registry, dispatcher, and instruction-scope
  tests affected by the change.
- [x] 4.2 Run workspace Clippy with warnings denied and the complete `misy-core` test suite.
