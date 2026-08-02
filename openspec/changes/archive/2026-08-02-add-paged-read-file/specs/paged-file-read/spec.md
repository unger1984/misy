## Purpose

Provide compatible, bounded line-range reads so agents can inspect focused portions of UTF-8 files
without routing ordinary source navigation through shell commands.

## ADDED Requirements

### Requirement: Optional line pagination contract
The core-owned `read_file` tool SHALL continue to require `path` and SHALL accept optional `offset`
and `limit` integer arguments. `offset` SHALL be one-based and at least 1, and `limit` SHALL be at
least 1 and at most 1000. Unknown arguments and values outside those ranges SHALL fail schema
validation before filesystem access.

#### Scenario: Existing call remains valid
- **WHEN** the model calls `read_file` with only a valid `path`
- **THEN** the tool returns the same bounded whole-file projection supported before this capability

#### Scenario: Explicit page is valid
- **WHEN** the model calls `read_file` with a valid `path`, `offset` of 101, and `limit` of 50
- **THEN** the tool accepts the call as a request for at most lines 101 through 150

#### Scenario: Invalid pagination is rejected
- **WHEN** the model supplies zero, a negative value, a non-integer value, a `limit` greater than 1000, or an unknown argument
- **THEN** the tool rejects the arguments without reading the target

### Requirement: Deterministic pagination defaults
When either pagination argument is present, `read_file` SHALL enter paged mode. In paged mode an
omitted `offset` SHALL default to 1 and an omitted `limit` SHALL default to 1000.

#### Scenario: Limit without offset
- **WHEN** the model requests `read_file` with `limit` of 40 and no `offset`
- **THEN** the returned page starts at line 1 and contains at most 40 lines

#### Scenario: Offset without limit
- **WHEN** the model requests `read_file` with `offset` of 201 and no `limit`
- **THEN** the returned page starts at line 201 and contains at most 1000 lines

### Requirement: Contiguous bounded line projection
In paged mode `read_file` SHALL return only the contiguous logical lines selected by the effective
offset and limit, subject to the existing bounded UTF-8 read ceiling. It SHALL preserve the selected
text content and SHALL NOT include text from preceding or subsequent lines as file content.

#### Scenario: Complete middle page
- **WHEN** a valid UTF-8 file has more lines after the requested range
- **THEN** the result contains at most the requested number of contiguous lines beginning at the effective offset

#### Scenario: Final partial page
- **WHEN** fewer lines remain than the effective limit
- **THEN** the result contains every remaining line and identifies that the end of the readable file was reached

#### Scenario: Offset after end of file
- **WHEN** the effective offset is greater than the number of readable lines
- **THEN** the tool succeeds with an empty page and identifies that the end of the readable file was reached

### Requirement: Actionable range metadata
Every paged result SHALL identify the effective starting line, the returned line range or empty-page
state, and whether another page is available. When another page is available, the result SHALL give
the exact `offset` for the next request. If the existing byte ceiling truncates the readable prefix,
the result SHALL distinguish that condition from reaching the physical end of the file.

#### Scenario: More lines are available
- **WHEN** a page beginning at line 101 returns 50 lines and at least one more readable line exists
- **THEN** the result identifies lines 101 through 150 and gives 151 as the next offset

#### Scenario: Read ceiling is reached
- **WHEN** the file extends beyond the existing bounded UTF-8 read ceiling
- **THEN** the paged result reports that the readable prefix was truncated and does not claim physical end of file

### Requirement: Existing file safety and instruction scope
Paged reads SHALL retain the existing `read_file` restrictions for missing targets, regular files,
UTF-8 content, and bounded reads. Hierarchical instruction preflight SHALL continue to resolve from
the requested `path` before any paged file access occurs.

#### Scenario: Invalid target fails without a page
- **WHEN** the target is missing, is not a regular file, or its readable bytes are not valid UTF-8
- **THEN** the tool returns the existing class of `read_file` error and no page content

#### Scenario: New instruction scope is activated first
- **WHEN** the requested path introduces a previously unseen applicable `AGENTS.md` scope
- **THEN** the core activates that scope and asks the model to retry before reading any part of the file
