## Purpose

Provide coding agents with a precise, validated way to edit an existing local text file without
rewriting the complete file or constructing a shell command.

## ADDED Requirements

### Requirement: Exact replacement tool contract
The core SHALL advertise a `StrReplaceFile` tool with required string arguments `path`, `old`, and
`new`, plus an optional boolean `replace_all` argument that defaults to `false`. The tool SHALL
reject unknown arguments.

#### Scenario: Tool is advertised
- **WHEN** the core builds the provider-visible tool definitions
- **THEN** `StrReplaceFile` is present with the defined argument schema

#### Scenario: Invalid arguments are rejected
- **WHEN** a call omits a required argument, supplies an argument of the wrong type, or supplies an
  unknown argument
- **THEN** the core rejects the call before accessing the target file

### Requirement: Unambiguous single replacement
When `replace_all` is false, `StrReplaceFile` SHALL replace `old` with `new` only when `old` occurs
exactly once in the target file.

#### Scenario: Unique fragment is replaced
- **WHEN** an existing regular UTF-8 file contains exactly one occurrence of a non-empty `old`
  value and `new` differs from `old`
- **THEN** the tool replaces that occurrence, preserves the rest of the file, and reports one
  replacement

#### Scenario: Fragment is absent
- **WHEN** the target file contains no occurrence of `old`
- **THEN** the tool returns an error and leaves the file unchanged

#### Scenario: Fragment is ambiguous
- **WHEN** the target file contains more than one occurrence of `old` and `replace_all` is false
- **THEN** the tool returns an error that asks for more identifying context or explicit
  `replace_all`, and leaves the file unchanged

### Requirement: Explicit replacement of every occurrence
When `replace_all` is true, `StrReplaceFile` SHALL replace every non-overlapping occurrence of
`old` and report the number of replacements.

#### Scenario: Repeated fragment is replaced everywhere
- **WHEN** a valid target file contains multiple occurrences of `old` and `replace_all` is true
- **THEN** every occurrence is replaced and the result reports the exact replacement count

#### Scenario: Replace all finds no occurrences
- **WHEN** a valid target file contains no occurrence of `old` and `replace_all` is true
- **THEN** the tool returns an error and leaves the file unchanged

### Requirement: Safe edit validation
`StrReplaceFile` SHALL edit only existing regular UTF-8 files whose complete input and projected
result are at most 4 MiB. Before allocating the replacement output, it SHALL count non-overlapping
matches and calculate the projected byte length with checked arithmetic. It SHALL reject arithmetic
overflow, a projected result above 4 MiB, an empty `old` value, and edits that cannot change the
file. Any validation failure SHALL occur without modifying the target.

#### Scenario: Empty old value is rejected
- **WHEN** `old` is empty
- **THEN** the tool returns an error without modifying the target file

#### Scenario: No-op edit is rejected
- **WHEN** `old` and `new` are identical
- **THEN** the tool returns an error without modifying the target file

#### Scenario: Invalid target is rejected
- **WHEN** the target is missing, is not a regular file, is not valid UTF-8, or exceeds 4 MiB
- **THEN** the tool returns an error without creating or modifying a file

#### Scenario: Output amplification is rejected
- **WHEN** match count and replacement lengths project a result above 4 MiB or overflow the byte
  length calculation
- **THEN** the tool returns an error before allocating replacement output and leaves the file
  unchanged

#### Scenario: Exact output boundary is accepted
- **WHEN** an otherwise valid edit projects a result of exactly 4 MiB
- **THEN** the tool applies the edit successfully

### Requirement: Hierarchical instruction preflight
`StrReplaceFile` SHALL use its `path` argument as a filesystem scope target so that all applicable
hierarchical `AGENTS.md` instructions are active before the edit executes.

#### Scenario: Nested instructions are discovered before editing
- **WHEN** a valid call targets a path governed by a previously undiscovered nested `AGENTS.md`
- **THEN** the core activates that instruction scope and retries the tool batch before any edit is
  performed
