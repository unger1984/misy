# Отчёт Task 3 — Контракт текста bootstrap-экрана

## Статус

Завершено.

## Изменённые файлы

- `src/main.rs`
  - Добавлен unit-тест `screen_copy_returns_bootstrap_text`.
  - Добавлен UI-независимый `ScreenCopy` с полями `title`, `body` и `footer` типа `&'static str`.
  - Добавлена минимальная функция `screen_copy(&AppState) -> ScreenCopy`, возвращающая заданный bootstrap-текст.
- `.ai/tasks/2026-07-23-0120-misy-tui-bootstrap/task-3-report.md`
  - Добавлен этот отчёт.

Рендеринг, управление жизненным циклом терминала, ввод и дополнительные UI-модели не добавлялись.

## TDD evidence

### RED

Сначала добавлен тест `screen_copy_returns_bootstrap_text`, проверяющий:

- `title == "misy"`;
- `body == "TUI shell booted"`;
- `footer == "Press q to quit"`.

До реализации выполнена команда:

```text
cargo test screen_copy_returns_bootstrap_text
```

Результат: ожидаемое падение компиляции `E0425` — функция `screen_copy` не найдена в области видимости (`src/main.rs:57:20`). Это подтверждает, что тест зависел от ещё не реализованного контракта.

### GREEN

После минимальной реализации выполнены команды:

```text
cargo test screen_copy_returns_bootstrap_text
cargo test
```

Результаты:

- focused test: 1 passed, 2 filtered, 0 failed;
- package tests: 3 passed, 0 failed.

## Коммиты

- `Task 3: add screen copy contract`

## Concerns

Нет.
