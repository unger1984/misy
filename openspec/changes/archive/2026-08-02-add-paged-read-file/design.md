## Context

See `proposal.md` for motivation. The provider-visible `read_file` schema currently contains only a
required `path`, while the filesystem helper returns a UTF-8 projection bounded to the first 4 MiB.
The tool registry owns validation, and core preflight derives hierarchical instruction scope from
the `path` argument before dispatch.

The new behavior must keep calls that provide only `path` unchanged. Paged output also needs to
distinguish a physical end of file from the existing 4 MiB readable-prefix boundary.

## Goals / Non-Goals

**Goals:**

- Add a small, provider-independent line pagination contract to the existing tool.
- Make every paged response sufficient to issue the next request deterministically.
- Reuse the current UTF-8, regular-file, byte-bound, and instruction-scope protections.
- Keep argument validation in the registry and file interpretation in the filesystem tool layer.

**Non-Goals:**

- Reading arbitrary ranges beyond the existing 4 MiB readable prefix.
- Tail-relative or negative offsets, multiple disjoint ranges, search, globbing, or binary reads.
- Changing the unpaged output format or introducing a provider protocol capability.

## Decisions

### Use optional one-based `offset` and `limit` arguments

`offset` names the first logical line and `limit` bounds the number of returned lines. Positive,
one-based positions match editor conventions and the local Oh My Pi reference. The schema caps
`limit` at 1000, matching the practical page bound in the local Kimi CLI reference.

When neither argument is supplied, dispatch follows the existing unpaged path. Supplying either
argument selects paged mode; absent values then become `offset = 1` and `limit = 1000`. This avoids
silently changing existing model calls while ensuring that every explicitly paged call is bounded.

Alternatives considered:

- Byte offsets allow direct seeking but are awkward for source-code navigation and can split UTF-8.
- Zero-based line offsets align with collection APIs but are less natural in model prompts and
  diagnostics.
- Negative offsets help tail logs but require a separate reverse-selection contract and are not
  needed for focused source reads.

### Page the existing bounded readable projection

The filesystem layer will separate bounded file loading from output formatting. Loading will expose
the valid UTF-8 prefix together with whether the physical file exceeded the existing 4 MiB ceiling.
Unpaged formatting will preserve its current output exactly. Paged formatting will select logical
lines from that projection and inspect one line beyond the page to determine continuation.

This keeps resource limits and compatibility stable. Streaming to arbitrary offsets was rejected
because it would replace the current bounded-input guarantee with potentially unbounded scanning;
that broader capability can be specified separately if real workloads require it.

### Emit compact machine-actionable metadata around paged content

A paged result will carry a concise header identifying the returned range, followed by the selected
text. Its terminal metadata will state exactly one of:

- `next_offset=<n>` when another readable line exists;
- `end_of_file` when the physical end is known;
- `read_boundary_reached` when the 4 MiB prefix ends before physical EOF.

An offset beyond the readable lines returns a successful empty page with the appropriate terminal
state. The exact punctuation is an implementation detail, but focused tests will lock down a stable,
unambiguous projection for the model.

Alternatives considered:

- Returning JSON would be easier to parse but would make source text harder for models to consume
  and would be inconsistent with the current text-result contract.
- Returning total line count requires scanning the complete physical file and conflicts with the
  existing input bound.

### Keep preflight path-scoped and unchanged

`offset` and `limit` do not affect filesystem scope. `ToolScopePolicy::PathArgument("path")` remains
authoritative, so instruction activation still completes before dispatch. No frontend or provider
changes are required because tool schemas and results already flow through the core contract.

## Risks / Trade-offs

- [A requested offset can lie beyond the 4 MiB readable prefix] → Report
  `read_boundary_reached` rather than incorrectly reporting physical EOF; document that arbitrary
  large-file traversal is not part of this change.
- [Line splitting can accidentally normalize CRLF or drop a final unterminated line] → Select byte
  spans at UTF-8 line boundaries and add focused LF, CRLF, empty-file, and unterminated-final-line
  tests.
- [Metadata could consume or resemble file text] → Use a stable delimited envelope and test files
  containing similar marker text so boundaries remain unambiguous.
- [Schema evolution could break cached provider assumptions] → The provider protocol already
  receives tool definitions per request; retain `path` and make both new fields optional.

## Migration Plan

No persisted-data or protocol migration is required. Ship the expanded schema and dispatcher
behavior together. Rollback restores the previous schema and unpaged dispatcher path; existing
`path`-only calls remain valid in both versions.
