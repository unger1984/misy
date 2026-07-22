# Task 5 Report — реальный terminal lifecycle

## Status

Выполнено. TUI запускает настоящий terminal shell, корректно выходит по `q` и выполняет восстановление терминала при нормальном завершении и при ошибке после takeover.

## Изменённые файлы

- `src/main.rs`
  - добавлены `CleanupStep` и чистая функция `restoration_plan`;
  - добавлены `draw_ui`, `restore_terminal`, `run_app` и fallible `main`;
  - terminal setup выполняется в порядке raw mode → alternate screen → скрытие курсора → `Terminal::new`;
  - при любой неудаче после setup запускается cleanup применимых шагов;
  - `restore_terminal` проходит весь план очистки и сохраняет первую ошибку только после попытки всех шагов;
  - `MISY_FAIL_AFTER_TAKEOVER=1` создаёт детерминированную ошибку после terminal takeover.
- `.ai/tasks/2026-07-23-0120-misy-tui-bootstrap/task-5-report.md`

## TDD evidence

### RED

До реализации `CleanupStep` и `restoration_plan` были добавлены два focused-теста:

- `restoration_plan_reverses_all_completed_terminal_setup_steps` ожидает обратную последовательность `ShowCursor`, `LeaveAlternateScreen`, `DisableRawMode`.
- `restoration_plan_omits_cleanup_for_setup_steps_that_did_not_complete` ожидает только cleanup действительно выполненных шагов.

Команда `cargo test restoration_plan` завершилась с кодом 101: тестовый код не скомпилировался ожидаемым образом, поскольку `restoration_plan` и `CleanupStep` ещё не существовали (`E0425` и `E0433`). Это зафиксировало RED до production-реализации.

### GREEN

После минимальной реализации чистого helper повторный запуск `cargo test restoration_plan` завершился успешно: `2 passed`, `5 filtered out`.

## Canonical quality gates

После форматирования исходника последовательно выполнены требуемые команды:

```text
cargo fmt --all -- --check                         PASS
cargo clippy --workspace --all-targets -- -D warnings  PASS
cargo test                                           PASS: 7 passed, 0 failed
```

Первый `fmt --check` до форматирования указал только на стандартные formatting differences; затем был применён `cargo fmt --all`, и полный canonical gate выше прошёл без предупреждений.

## Smoke verification

### Positive

В pseudo-terminal выполнено:

```text
printf 'q' | script -q /dev/null cargo run
```

Процесс завершился с кодом 0 после передачи `q`. Это проверило запуск TUI и normal exit path.

### Negative

В pseudo-terminal выполнено:

```text
script -q /dev/null env MISY_FAIL_AFTER_TAKEOVER=1 cargo run
```

Процесс завершился с кодом 1 и вывел ожидаемую ошибку:

```text
MISY_FAIL_AFTER_TAKEOVER requested a post-takeover failure
```

Команда вернула управление shell после terminal takeover/error path, что подтверждает исполняемость негативного smoke пути и cleanup sequencing.

## Commits created

- `82d3ce3` — `Implement TUI terminal lifecycle` (implementation).
- `Record Task 5 report` (this report).

## Concerns

Нет. `script` предоставил pseudo-terminal для smoke-проверок; оболочка harness не выделила интерактивный PTY самому process runner, что было явно отмечено runner, но оба сценария были выполнены через `script` и дали ожидаемые коды завершения.
