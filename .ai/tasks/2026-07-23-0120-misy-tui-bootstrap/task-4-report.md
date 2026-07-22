# Task 4 Report — Key Mapping

## Status
Complete.

## Files changed
- `src/main.rs`
  - Added `action_from_key(crossterm::event::KeyCode) -> Option<Action>`.
  - Added focused unit tests for `q` and an unrelated key.
- `.ai/tasks/2026-07-23-0120-misy-tui-bootstrap/task-4-report.md`
  - Added this report.

## Implementation
`action_from_key` is a pure helper with no terminal I/O. It maps only `KeyCode::Char('q')` to `Some(Action::Quit)` and returns `None` for all other key codes.

No event loop, rendering, or changes to `AppState` or `ScreenCopy` were made.

## TDD evidence

### RED
After adding the two tests and before adding production code, ran:

```text
cargo test key_maps --bin misy
```

Result: failed as expected during compilation because `action_from_key` was not defined:

```text
error[E0425]: cannot find function `action_from_key` in this scope
  --> src/main.rs:84:20
```

The same expected missing-function error occurred at both new test call sites.

### GREEN
Implemented the minimal `match` helper, then ran:

```text
cargo test key --bin misy
```

Result:

```text
cargo test: 2 passed (1 suite, 3 filtered, 0.00s)
```

## Tests run

```text
cargo test key --bin misy
```
- Passed: 2 focused key-mapping tests.

```text
cargo test
```
- Passed: 5 tests in 1 suite.

## Commits created
One Task 4 commit includes the implementation, tests, and this report.

## Concerns
None.
