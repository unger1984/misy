## Context

See [proposal.md](proposal.md) for motivation and
[specs/str-replace-file/spec.md](specs/str-replace-file/spec.md) for observable behavior. Local
tools are declared and dispatched by `misy-core`; filesystem calls run through bounded blocking
tasks, and path-bearing tools participate in hierarchical instruction preflight. The provider
contract already carries JSON Schema function tools, so this capability does not require a new
wire shape.

The current `read_file` helper may return a truncated prefix with a marker. An edit operation must
instead obtain the complete file or fail, because applying a replacement to a projection would
corrupt the target.

## Goals / Non-Goals

**Goals:**

- Keep the edit implementation owned by the Rust core and compatible with every current provider.
- Make validation complete before the first write and return actionable failure messages.
- Reuse the existing 4 MiB filesystem boundary for both complete input and projected output without
  editing truncated content or allocating an oversized result.
- Keep `tools.rs` from absorbing another complete filesystem operation.

**Non-Goals:**

- Creating, deleting, moving, or renaming files.
- Parsing unified diffs or Codex `apply_patch` grammar.
- Applying multiple ordered edits in one call.
- Adding permissions, approval UI, diff rendering, undo, or concurrent file locking.
- Changing the provider protocol or provider plugins.

## Decisions

### Use a JSON Schema string-replacement tool

The provider-visible name is `StrReplaceFile`, following the Kimi-style tool name already familiar
to supported model families. Its flat arguments are `path`, `old`, `new`, and `replace_all`; the
flat shape keeps one edit easy to generate and validate through the existing tool protocol.

The alternative was Codex `apply_patch`. Its full grammar, multi-file operations, and freeform
custom-tool transport would expand this change into provider translation work. A shell-backed
patch command was also rejected because it would add an external executable assumption and make
path scoping less explicit.

### Require uniqueness for the default operation

With `replace_all` absent or false, the implementation counts non-overlapping matches before
writing and succeeds only for exactly one match. This is stricter than replacing the first match:
an agent must provide more surrounding context rather than silently editing the wrong repeated
fragment. Explicit `replace_all` remains available for intentional bulk replacement.

### Separate complete edit reads from display reads

A small `tools/string_replace.rs` module owns argument extraction, bounded blocking execution, and
replacement result construction. The filesystem layer exposes a complete bounded UTF-8 read path
that rejects oversized input; `read_file` may continue using its existing truncated display path.
This avoids duplicating regular-file, UTF-8, and size validation while preventing partial-file
edits.

The alternative was to reuse the rendered `read_file` result. That output can contain a synthetic
truncation marker and therefore is not valid edit input.

### Bound projected output before allocation

The implementation first counts non-overlapping matches without constructing replacement output.
It then computes the projected byte length as
`input_len - match_count * old_len + match_count * new_len`, using checked multiplication,
subtraction, and addition. Arithmetic failure or a projected length above the shared 4 MiB text
file limit returns an error before output allocation or filesystem mutation. A result exactly at
the limit is valid. Only after the check passes may the implementation allocate and construct the
replacement string.

The alternative was to rely on the input-file and provider-frame limits. Those bounds do not
prevent a small input containing many short matches from expanding into a much larger derived
result.

### Reuse existing dispatch and instruction-scope boundaries

The registry gains the new schema, the dispatcher delegates the call to the new module, and
`builtin_scope_policy` maps `StrReplaceFile` to `PathArgument("path")`. No orchestration moves to
the TUI or provider plugins, and generic tool-call rendering remains sufficient for this slice.

## Risks / Trade-offs

- [The file may change between reading and writing] → This small slice provides the same
  single-process filesystem guarantees as `write_file`; optimistic concurrency or file locking can
  be added as a separate capability if real usage exposes the race.
- [A replacement can amplify a bounded input] → Count matches and validate the projected byte
  length with checked arithmetic against the same 4 MiB limit before allocating output.
- [Large exact fragments increase tool-call size] → Existing provider frame and context limits
  remain authoritative for the arguments themselves.
- [Replacement count semantics can surprise on overlapping text] → Define and test conventional
  non-overlapping string replacement, matching Rust string replacement behavior.
- [Generic TUI rendering does not show a rich diff] → Return the path and replacement count now;
  specialized diff presentation is deliberately deferred.

## Migration Plan

No stored data or external protocol migrates. Shipping the new core version advertises the tool to
all supported providers. Rollback removes the tool definition and dispatcher route; existing
session history can still replay its generic historical call and result records.
