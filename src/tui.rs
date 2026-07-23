use std::io;

use arboard::Clipboard;
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        MouseButton, MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
        supports_keyboard_enhancement,
    },
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use unicode_width::UnicodeWidthChar;

use crate::app::{Action, AppState, FocusedPane, fixture_sections};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SetupState {
    raw_mode_enabled: bool,
    alternate_screen_entered: bool,
    mouse_capture_enabled: bool,
    cursor_hidden: bool,
    bracketed_paste_enabled: bool,
    keyboard_enhancement_enabled: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CleanupStep {
    ShowCursor,
    DisableMouseCapture,
    LeaveAlternateScreen,
    DisableBracketedPaste,
    PopKeyboardEnhancement,
    DisableRawMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PaneAreas {
    sections: Rect,
    messages: Rect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct UiAreas {
    panes: PaneAreas,
    composer: Rect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ComposerLineMetrics {
    total_rows: usize,
    cursor_row: usize,
    cursor_column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClipboardImage {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClipboardError {
    Unavailable(String),
}

#[derive(Debug, Default)]
struct ViewState {
    sections: ListState,
    messages: ListState,
    notification: Option<String>,
}

pub fn run_app() -> io::Result<()> {
    let mut setup = SetupState::default();
    let mut stdout = io::stdout();

    enable_raw_mode()?;
    setup.raw_mode_enabled = true;

    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        return Err(merge_cleanup_error(error, restore_terminal(setup)));
    }
    setup.alternate_screen_entered = true;

    if let Err(error) = execute!(stdout, EnableMouseCapture) {
        return Err(merge_cleanup_error(error, restore_terminal(setup)));
    }
    setup.mouse_capture_enabled = true;

    if let Err(error) = execute!(stdout, EnableBracketedPaste) {
        return Err(merge_cleanup_error(error, restore_terminal(setup)));
    }
    setup.bracketed_paste_enabled = true;

    if let Err(error) = execute!(stdout, Hide) {
        return Err(merge_cleanup_error(error, restore_terminal(setup)));
    }
    setup.cursor_hidden = true;

    let keyboard_enhancement_supported = supports_keyboard_enhancement().unwrap_or(false);
    if keyboard_enhancement_supported {
        if let Err(error) = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        ) {
            return Err(merge_cleanup_error(error, restore_terminal(setup)));
        }
        setup.keyboard_enhancement_enabled = true;
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => return Err(merge_cleanup_error(error, restore_terminal(setup))),
    };

    if let Err(error) = terminal.clear() {
        return Err(merge_cleanup_error(error, restore_terminal(setup)));
    }

    let mut state = AppState::new(fixture_sections());
    let mut view = ViewState::default();
    let mut last_ui_areas = UiAreas::default();

    let runtime_result = (|| -> io::Result<()> {
        while state.running {
            terminal.draw(|frame| {
                last_ui_areas = draw_ui(frame, &mut state, &mut view);
            })?;

            let event = event::read()?;
            handle_terminal_event(event, &mut state, &mut view, &last_ui_areas);
        }
        Ok(())
    })();

    drop(terminal);
    let cleanup_result = restore_terminal(setup);

    match (runtime_result, cleanup_result) {
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup_error)) => Err(io::Error::other(format!(
            "runtime failed: {error}; cleanup failed: {cleanup_error}"
        ))),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn merge_cleanup_error(primary: io::Error, cleanup: io::Result<()>) -> io::Error {
    match cleanup {
        Ok(()) => primary,
        Err(cleanup_error) => io::Error::other(format!(
            "terminal setup failed: {primary}; cleanup failed: {cleanup_error}"
        )),
    }
}

fn restoration_plan(state: SetupState) -> Vec<CleanupStep> {
    let mut steps = Vec::with_capacity(6);
    if state.cursor_hidden {
        steps.push(CleanupStep::ShowCursor);
    }
    if state.mouse_capture_enabled {
        steps.push(CleanupStep::DisableMouseCapture);
    }
    if state.alternate_screen_entered {
        steps.push(CleanupStep::LeaveAlternateScreen);
    }
    if state.bracketed_paste_enabled {
        steps.push(CleanupStep::DisableBracketedPaste);
    }
    if state.keyboard_enhancement_enabled {
        steps.push(CleanupStep::PopKeyboardEnhancement);
    }
    if state.raw_mode_enabled {
        steps.push(CleanupStep::DisableRawMode);
    }
    steps
}

fn restore_terminal(state: SetupState) -> io::Result<()> {
    let mut first_error = None;
    let mut stdout = io::stdout();

    for step in restoration_plan(state) {
        let result = match step {
            CleanupStep::ShowCursor => execute!(stdout, Show),
            CleanupStep::DisableMouseCapture => execute!(stdout, DisableMouseCapture),
            CleanupStep::LeaveAlternateScreen => execute!(stdout, LeaveAlternateScreen),
            CleanupStep::DisableBracketedPaste => execute!(stdout, DisableBracketedPaste),
            CleanupStep::PopKeyboardEnhancement => execute!(stdout, PopKeyboardEnhancementFlags),
            CleanupStep::DisableRawMode => disable_raw_mode(),
        };

        if let Err(error) = result
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }

    first_error.map_or(Ok(()), Err)
}

#[cfg(target_os = "linux")]
// TODO(linux): implement OSC 5522 image paste
fn platform_clipboard_image_hint() {}

#[cfg(target_os = "windows")]
// TODO(windows): implement OSC 5522 image paste
fn platform_clipboard_image_hint() {}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn platform_clipboard_image_hint() {}

fn clipboard_error(error: arboard::Error) -> ClipboardError {
    ClipboardError::Unavailable(error.to_string())
}

fn clipboard_image() -> Result<Option<ClipboardImage>, ClipboardError> {
    platform_clipboard_image_hint();
    let mut clipboard = Clipboard::new().map_err(clipboard_error)?;
    match clipboard.get_image() {
        Ok(image) => Ok(Some(ClipboardImage {
            width: image.width,
            height: image.height,
            rgba: image.bytes.into_owned(),
        })),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(error) => Err(clipboard_error(error)),
    }
}

fn clipboard_text() -> Result<Option<String>, ClipboardError> {
    let mut clipboard = Clipboard::new().map_err(clipboard_error)?;
    match clipboard.get_text() {
        Ok(text) => Ok(Some(text)),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(error) => Err(clipboard_error(error)),
    }
}

fn paste_action(
    image_result: Result<Option<ClipboardImage>, ClipboardError>,
    text_result: Result<Option<String>, ClipboardError>,
) -> Result<Option<Action>, ClipboardError> {
    if let Some(image) = image_result? {
        return Ok(Some(Action::InsertImage {
            width: image.width,
            height: image.height,
            rgba: image.rgba,
        }));
    }

    if let Some(text) = text_result? {
        return Ok(Some(Action::InsertText(text)));
    }

    Ok(None)
}

fn handle_clipboard_paste(state: &mut AppState, view: &mut ViewState) {
    let paste_result = match clipboard_image() {
        Ok(Some(image)) => paste_action(Ok(Some(image)), Ok(None)),
        Ok(None) => paste_action(Ok(None), clipboard_text()),
        Err(error) => Err(error),
    };

    match paste_result {
        Ok(Some(action)) => {
            state.apply(action);
            view.notification = None;
        }
        Ok(None) => {
            view.notification = None;
        }
        Err(error) => {
            // TODO(task-3): route clipboard notifications through a shared status area when TUI gets one.
            view.notification = Some(format!(
                "Clipboard paste failed: {}",
                match error {
                    ClipboardError::Unavailable(message) => message,
                }
            ));
        }
    }
}

fn composer_action_from_key(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Tab => Some(Action::CycleFocus),
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
            Some(Action::InsertLineBreak)
        }
        KeyCode::Enter => Some(Action::SubmitComposer),
        KeyCode::Char(ch)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(Action::InsertText(ch.to_string()))
        }
        KeyCode::Left => Some(Action::MoveCursorLeft),
        KeyCode::Right => Some(Action::MoveCursorRight),
        KeyCode::Backspace => Some(Action::Backspace),
        KeyCode::Delete => Some(Action::Delete),
        KeyCode::PageUp => Some(Action::ScrollComposerUp),
        KeyCode::PageDown => Some(Action::ScrollComposerDown),
        KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Action::ScrollComposerUp)
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Action::ScrollComposerDown)
        }
        KeyCode::Up | KeyCode::Down => None,
        _ => None,
    }
}

fn handle_terminal_event(
    event: Event,
    state: &mut AppState,
    view: &mut ViewState,
    last_ui_areas: &UiAreas,
) {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            if state.focused_pane == FocusedPane::Composer
                && key.code == KeyCode::Char('v')
                && key.modifiers.contains(KeyModifiers::CONTROL)
            {
                handle_clipboard_paste(state, view);
                return;
            }

            let action = if state.focused_pane == FocusedPane::Composer {
                composer_action_from_key(key)
            } else {
                action_from_key_event(key)
            };

            if let Some(action) = action {
                state.apply(action);
            }
        }
        Event::Paste(text) => {
            if state.focused_pane == FocusedPane::Composer {
                state.apply(Action::InsertText(text));
                view.notification = None;
            }
        }
        Event::Mouse(mouse) => {
            if let Some(action) = composer_action_from_mouse_event(mouse, last_ui_areas.composer) {
                state.apply(action);
            } else if let Some(action) = action_from_mouse_event(
                mouse,
                &last_ui_areas.panes,
                view.sections.offset(),
                view.messages.offset(),
                state,
            ) {
                state.apply(action);
            }
        }
        _ => {}
    }
}

fn pane_areas(area: Rect) -> PaneAreas {
    let [sections, messages] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)]).areas(area);
    PaneAreas { sections, messages }
}

fn ui_areas(area: Rect, composer_content_rows: usize) -> UiAreas {
    if area.height == 0 {
        return UiAreas::default();
    }

    if area.height < 3 {
        return UiAreas {
            panes: PaneAreas::default(),
            composer: area,
        };
    }

    let composer_height = composer_content_rows.clamp(1, 10).saturating_add(2);
    let composer_height = composer_height.min(usize::from(area.height)) as u16;
    let upper_height = area.height.saturating_sub(composer_height);
    let upper = Rect::new(area.x, area.y, area.width, upper_height);

    UiAreas {
        panes: pane_areas(upper),
        composer: Rect::new(
            area.x,
            area.y.saturating_add(upper_height),
            area.width,
            composer_height,
        ),
    }
}

fn composer_line_metrics(text: &str, width: usize, cursor: usize) -> ComposerLineMetrics {
    let width = width.max(1);
    let cursor = cursor.min(text.len());
    let mut total_rows = 0usize;
    let mut cursor_row = 0usize;
    let mut cursor_column = 0usize;
    let mut current_width = 0usize;

    for (idx, ch) in text.char_indices() {
        if idx == cursor {
            cursor_row = total_rows;
            cursor_column = current_width;
        }

        if ch == '\n' {
            total_rows += 1;
            current_width = 0;
            continue;
        }

        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width > 0 && current_width.saturating_add(char_width) > width {
            total_rows += 1;
            current_width = 0;
        }
        current_width = current_width.saturating_add(char_width);
    }

    if cursor == text.len() {
        cursor_row = total_rows;
        cursor_column = current_width;
    }

    ComposerLineMetrics {
        total_rows: total_rows.saturating_add(1),
        cursor_row,
        cursor_column,
    }
}
fn composer_display_text(state: &AppState) -> (String, usize) {
    let text = state.composer_text();
    let mut displayed = String::with_capacity(text.len());
    let mut raw_index = 0;
    let mut cursor = 0;

    while raw_index < text.len() {
        if raw_index == state.composer_cursor() {
            cursor = displayed.len();
        }
        if let Some((end, id)) = image_token_at(text, raw_index)
            && let Some(attachment) = state
                .attachments()
                .iter()
                .find(|attachment| attachment.id == id)
        {
            displayed.push_str(&format!(
                "[Image #{id}, {}x{}]",
                attachment.width, attachment.height
            ));
            raw_index = end;
            continue;
        }
        let Some(ch) = text[raw_index..].chars().next() else {
            break;
        };
        displayed.push(ch);
        raw_index += ch.len_utf8();
    }
    if raw_index == state.composer_cursor() {
        cursor = displayed.len();
    }

    (displayed, cursor)
}

fn image_token_at(text: &str, start: usize) -> Option<(usize, u64)> {
    let rest = text.get(start..)?;
    let digits = rest.strip_prefix("[Image #")?;
    let close = digits.find(']')?;
    let number = digits.get(..close)?;
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let id = number.parse().ok()?;
    Some((start + "[Image #".len() + close + 1, id))
}

fn composer_content_height(text: &str, width: usize) -> usize {
    composer_line_metrics(text, width, text.len())
        .total_rows
        .clamp(1, 10)
}

fn composer_cursor_row(text: &str, width: usize, cursor: usize) -> usize {
    composer_line_metrics(text, width, cursor).cursor_row
}

fn composer_viewport_top(text: &str, width: usize, cursor: usize, current_scroll: usize) -> usize {
    let cursor_row = composer_cursor_row(text, width, cursor);
    let viewport_height = 10;

    if cursor_row < current_scroll {
        cursor_row
    } else if cursor_row >= current_scroll.saturating_add(viewport_height) {
        cursor_row.saturating_add(1).saturating_sub(viewport_height)
    } else {
        current_scroll
    }
}

fn draw_ui(frame: &mut Frame, state: &mut AppState, view: &mut ViewState) -> UiAreas {
    let frame_area = frame.area();
    if frame_area.height == 0 {
        return UiAreas::default();
    }

    frame.render_widget(Clear, frame_area);

    let (composer_text, composer_cursor) = composer_display_text(state);
    let composer_width =
        usize::from(
            frame_area
                .width
                .saturating_sub(if frame_area.height >= 3 { 2 } else { 0 }),
        );
    let composer_height = composer_content_height(&composer_text, composer_width);
    let areas = ui_areas(frame_area, composer_height);

    view.sections
        .select((!state.sections.is_empty()).then_some(state.selected_section));
    view.messages
        .select((!state.current_messages().is_empty()).then_some(state.selected_message));

    if areas.panes.sections.height > 0 {
        let section_content_width = usize::from(areas.panes.sections.width.saturating_sub(2));
        let section_viewport_height = usize::from(areas.panes.sections.height.saturating_sub(2));
        let section_items: Vec<ListItem> = pad_items_to_viewport(
            if state.sections.is_empty() {
                vec![ListItem::new(padded_cell(
                    "No sections",
                    section_content_width,
                ))]
            } else {
                state
                    .sections
                    .iter()
                    .map(|section| {
                        ListItem::new(padded_cell(section.title.as_str(), section_content_width))
                    })
                    .collect()
            },
            section_viewport_height,
            section_content_width,
        );

        let sections_block = Block::default()
            .title(pane_title(
                "Sections",
                state.focused_pane == FocusedPane::Sections,
            ))
            .borders(Borders::ALL)
            .border_style(focus_style(state.focused_pane == FocusedPane::Sections));

        frame.render_widget(
            Clear,
            areas.panes.sections.inner(Margin {
                vertical: 1,
                horizontal: 1,
            }),
        );
        frame.render_stateful_widget(
            List::new(section_items)
                .block(sections_block)
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
            areas.panes.sections,
            &mut view.sections,
        );

        render_scrollbar(
            frame,
            areas.panes.sections,
            state.sections.len(),
            scrollbar_position(
                (!state.sections.is_empty()).then_some(state.selected_section),
                view.sections.offset(),
            ),
        );

        let message_content_width = usize::from(areas.panes.messages.width.saturating_sub(2));
        let message_viewport_height = usize::from(areas.panes.messages.height.saturating_sub(2));
        let message_items: Vec<ListItem> = pad_items_to_viewport(
            if state.current_messages().is_empty() {
                vec![ListItem::new(padded_cell(
                    "No messages",
                    message_content_width,
                ))]
            } else {
                state
                    .current_messages()
                    .iter()
                    .map(|message| {
                        ListItem::new(padded_cell(message.as_str(), message_content_width))
                    })
                    .collect()
            },
            message_viewport_height,
            message_content_width,
        );

        let messages_block = Block::default()
            .title(pane_title(
                "Messages",
                state.focused_pane == FocusedPane::Messages,
            ))
            .borders(Borders::ALL)
            .border_style(focus_style(state.focused_pane == FocusedPane::Messages));
        frame.render_widget(
            Clear,
            areas.panes.messages.inner(Margin {
                vertical: 1,
                horizontal: 1,
            }),
        );
        frame.render_stateful_widget(
            List::new(message_items)
                .block(messages_block)
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
            areas.panes.messages,
            &mut view.messages,
        );

        render_scrollbar(
            frame,
            areas.panes.messages,
            state.current_messages().len(),
            scrollbar_position(
                (!state.current_messages().is_empty()).then_some(state.selected_message),
                view.messages.offset(),
            ),
        );
    }

    let composer_scroll = composer_viewport_top(
        &composer_text,
        composer_width,
        composer_cursor,
        state.composer_scroll(),
    );
    state.set_composer_scroll(composer_scroll);

    let composer_focused = state.focused_pane == FocusedPane::Composer;
    let composer_title = match view.notification.as_deref() {
        Some(notification) => format!(
            "{} — {}",
            pane_title("Composer", composer_focused),
            notification
        ),
        None => pane_title("Composer", composer_focused),
    };
    let composer_block = if areas.composer.height >= 3 {
        Block::default()
            .title(composer_title)
            .borders(Borders::ALL)
            .border_style(focus_style(composer_focused))
    } else {
        Block::default()
    };
    let composer = Paragraph::new(composer_text.as_str())
        .block(composer_block)
        .wrap(Wrap { trim: false })
        .scroll((state.composer_scroll() as u16, 0));
    frame.render_widget(composer, areas.composer);

    if composer_focused {
        let metrics = composer_line_metrics(&composer_text, composer_width, composer_cursor);
        let inner = areas.composer.inner(Margin {
            vertical: u16::from(areas.composer.height >= 3),
            horizontal: u16::from(areas.composer.height >= 3),
        });
        let cursor_row = metrics.cursor_row.saturating_sub(state.composer_scroll());
        if cursor_row < usize::from(inner.height) {
            frame.set_cursor_position((
                inner.x.saturating_add(metrics.cursor_column as u16),
                inner.y.saturating_add(cursor_row as u16),
            ));
        }
    }

    areas
}

fn pad_items_to_viewport(
    mut items: Vec<ListItem<'static>>,
    viewport_height: usize,
    width: usize,
) -> Vec<ListItem<'static>> {
    while items.len() < viewport_height {
        items.push(ListItem::new(" ".repeat(width)));
    }
    items
}

fn padded_cell(text: &str, width: usize) -> String {
    let mut cell = text.chars().take(width).collect::<String>();
    let visible_len = cell.chars().count();
    if visible_len < width {
        cell.push_str(&" ".repeat(width - visible_len));
    }
    cell
}

fn pane_title(label: &str, focused: bool) -> String {
    if focused {
        format!("▶ {label}")
    } else {
        label.to_string()
    }
}

fn scrollbar_position(selected: Option<usize>, offset: usize) -> usize {
    selected.unwrap_or(offset)
}

fn render_scrollbar(frame: &mut Frame, pane: Rect, content_length: usize, position: usize) {
    let viewport_height = usize::from(pane.height.saturating_sub(2));
    if content_length <= viewport_height || viewport_height == 0 {
        return;
    }

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None);
    let mut scrollbar_state = ScrollbarState::new(content_length)
        .position(position)
        .viewport_content_length(viewport_height);

    frame.render_stateful_widget(
        scrollbar,
        pane.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut scrollbar_state,
    );
}

fn focus_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn action_from_key_event(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Up => Some(Action::MoveUp),
        KeyCode::Down => Some(Action::MoveDown),
        KeyCode::Left => Some(Action::FocusLeft),
        KeyCode::Right => Some(Action::FocusRight),
        KeyCode::Tab => Some(Action::CycleFocus),
        KeyCode::Enter => Some(Action::Activate),
        _ => None,
    }
}

fn action_from_mouse_event(
    event: MouseEvent,
    panes: &PaneAreas,
    section_offset: usize,
    message_offset: usize,
    state: &AppState,
) -> Option<Action> {
    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if panes.sections.contains((event.column, event.row).into()) {
                item_index_in_pane(
                    panes.sections,
                    event.row,
                    state.sections.len(),
                    section_offset,
                )
                .map(Action::SelectSection)
                .or(Some(Action::FocusLeft))
            } else if panes.messages.contains((event.column, event.row).into()) {
                item_index_in_pane(
                    panes.messages,
                    event.row,
                    state.current_messages().len(),
                    message_offset,
                )
                .map(Action::SelectMessage)
                .or(Some(Action::FocusRight))
            } else {
                None
            }
        }
        MouseEventKind::ScrollUp => {
            pane_for_point(event.column, event.row, panes).map(Action::MoveUpIn)
        }
        MouseEventKind::ScrollDown => {
            pane_for_point(event.column, event.row, panes).map(Action::MoveDownIn)
        }
        _ => None,
    }
}

fn composer_action_from_mouse_event(event: MouseEvent, composer: Rect) -> Option<Action> {
    (event.kind == MouseEventKind::Down(MouseButton::Left)
        && composer.contains((event.column, event.row).into()))
    .then_some(Action::FocusComposer)
}

fn pane_for_point(column: u16, row: u16, panes: &PaneAreas) -> Option<FocusedPane> {
    if panes.sections.contains((column, row).into()) {
        Some(FocusedPane::Sections)
    } else if panes.messages.contains((column, row).into()) {
        Some(FocusedPane::Messages)
    } else {
        None
    }
}

fn item_index_in_pane(pane: Rect, row: u16, item_count: usize, offset: usize) -> Option<usize> {
    if pane.height < 2 {
        return None;
    }

    let content_top = pane.y.saturating_add(1);
    let content_bottom_exclusive = pane.y.saturating_add(pane.height.saturating_sub(1));

    if row < content_top || row >= content_bottom_exclusive {
        return None;
    }

    let relative_row = usize::from(row - content_top);
    let index = offset.saturating_add(relative_row);
    (index < item_count).then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Section;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn q_key_maps_to_quit() {
        let key = KeyEvent::new(KeyCode::Char('q'), event::KeyModifiers::NONE);
        assert_eq!(action_from_key_event(key), Some(Action::Quit));
    }

    #[test]
    fn arrow_and_tab_keys_map_to_navigation_actions() {
        assert_eq!(
            action_from_key_event(KeyEvent::new(KeyCode::Up, event::KeyModifiers::NONE)),
            Some(Action::MoveUp)
        );
        assert_eq!(
            action_from_key_event(KeyEvent::new(KeyCode::Down, event::KeyModifiers::NONE)),
            Some(Action::MoveDown)
        );
        assert_eq!(
            action_from_key_event(KeyEvent::new(KeyCode::Left, event::KeyModifiers::NONE)),
            Some(Action::FocusLeft)
        );
        assert_eq!(
            action_from_key_event(KeyEvent::new(KeyCode::Right, event::KeyModifiers::NONE)),
            Some(Action::FocusRight)
        );
        assert_eq!(
            action_from_key_event(KeyEvent::new(KeyCode::Tab, event::KeyModifiers::NONE)),
            Some(Action::CycleFocus)
        );
    }

    #[test]
    fn enter_key_maps_to_activate() {
        let key = KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE);
        assert_eq!(action_from_key_event(key), Some(Action::Activate));
    }
    #[test]
    fn composer_key_events_map_enter_and_shift_enter_to_distinct_actions() {
        assert_eq!(
            composer_action_from_key(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE)),
            Some(Action::SubmitComposer)
        );
        assert_eq!(
            composer_action_from_key(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::SHIFT)),
            Some(Action::InsertLineBreak)
        );
    }
    #[test]
    fn composer_key_events_insert_plain_q_instead_of_quitting() {
        assert_eq!(
            composer_action_from_key(KeyEvent::new(KeyCode::Char('q'), event::KeyModifiers::NONE)),
            Some(Action::InsertText("q".into()))
        );
    }

    #[test]
    fn pane_areas_split_the_frame_into_two_columns() {
        let panes = pane_areas(Rect::new(0, 0, 100, 30));
        assert!(panes.sections.width > 0);
        assert!(panes.messages.width > 0);
        assert_eq!(panes.sections.x, 0);
        assert!(panes.messages.x > panes.sections.x);
    }

    #[test]
    fn empty_sections_render_placeholder() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(vec![]);
        let mut view = ViewState::default();

        terminal
            .draw(|frame| {
                let _ = draw_ui(frame, &mut state, &mut view);
            })
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("No sections"));
    }

    #[test]
    fn empty_messages_render_placeholder() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(vec![Section {
            title: "Empty".into(),
            messages: vec![],
        }]);
        let mut view = ViewState::default();

        terminal
            .draw(|frame| {
                let _ = draw_ui(frame, &mut state, &mut view);
            })
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("No messages"));
    }

    #[test]
    fn restoration_plan_releases_mouse_capture_before_leaving_terminal() {
        assert_eq!(
            restoration_plan(SetupState {
                raw_mode_enabled: true,
                alternate_screen_entered: true,
                mouse_capture_enabled: true,
                cursor_hidden: true,
                bracketed_paste_enabled: true,
                keyboard_enhancement_enabled: true,
            }),
            vec![
                CleanupStep::ShowCursor,
                CleanupStep::DisableMouseCapture,
                CleanupStep::LeaveAlternateScreen,
                CleanupStep::DisableBracketedPaste,
                CleanupStep::PopKeyboardEnhancement,
                CleanupStep::DisableRawMode,
            ]
        );
    }

    #[test]
    fn restoration_plan_handles_partial_setup_prefixes() {
        assert_eq!(
            restoration_plan(SetupState {
                raw_mode_enabled: true,
                ..SetupState::default()
            }),
            vec![CleanupStep::DisableRawMode]
        );
        assert_eq!(
            restoration_plan(SetupState {
                raw_mode_enabled: true,
                alternate_screen_entered: true,
                ..SetupState::default()
            }),
            vec![
                CleanupStep::LeaveAlternateScreen,
                CleanupStep::DisableRawMode
            ]
        );
        assert_eq!(
            restoration_plan(SetupState {
                raw_mode_enabled: true,
                alternate_screen_entered: true,
                mouse_capture_enabled: true,
                ..SetupState::default()
            }),
            vec![
                CleanupStep::DisableMouseCapture,
                CleanupStep::LeaveAlternateScreen,
                CleanupStep::DisableRawMode,
            ]
        );
    }
    #[test]
    fn paste_action_prefers_clipboard_image_over_text() {
        let image = ClipboardImage {
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };

        assert_eq!(
            paste_action(Ok(Some(image.clone())), Ok(Some("ignored".into()))),
            Ok(Some(Action::InsertImage {
                width: image.width,
                height: image.height,
                rgba: image.rgba,
            }))
        );
    }

    #[test]
    fn paste_action_uses_clipboard_text_when_image_is_absent() {
        assert_eq!(
            paste_action(Ok(None), Ok(Some("hello".into()))),
            Ok(Some(Action::InsertText("hello".into())))
        );
    }

    #[test]
    fn paste_action_returns_error_without_text_fallback_or_clear_action() {
        let error = ClipboardError::Unavailable("image read failed".into());

        assert_eq!(
            paste_action(Err(error.clone()), Ok(Some("hello".into()))),
            Err(error)
        );
    }

    #[test]
    fn paste_event_inserts_text_only_when_composer_is_focused() {
        let mut focused_state = AppState::new(fixture_sections());
        focused_state.focused_pane = FocusedPane::Composer;
        let mut focused_view = ViewState::default();

        handle_terminal_event(
            Event::Paste("hello".into()),
            &mut focused_state,
            &mut focused_view,
            &UiAreas::default(),
        );

        assert_eq!(focused_state.composer_text(), "hello");

        let mut unfocused_state = AppState::new(fixture_sections());
        let mut unfocused_view = ViewState::default();

        handle_terminal_event(
            Event::Paste("ignored".into()),
            &mut unfocused_state,
            &mut unfocused_view,
            &UiAreas::default(),
        );

        assert_eq!(unfocused_state.composer_text(), "");
    }

    #[test]
    fn bracketed_paste_restoration_disables_bracketed_paste_before_raw_mode() {
        assert_eq!(
            restoration_plan(SetupState {
                raw_mode_enabled: true,
                alternate_screen_entered: true,
                mouse_capture_enabled: true,
                cursor_hidden: true,
                keyboard_enhancement_enabled: false,
                bracketed_paste_enabled: true,
            }),
            vec![
                CleanupStep::ShowCursor,
                CleanupStep::DisableMouseCapture,
                CleanupStep::LeaveAlternateScreen,
                CleanupStep::DisableBracketedPaste,
                CleanupStep::DisableRawMode,
            ]
        );
    }

    #[test]
    fn click_on_section_item_focuses_sections_and_selects_clicked_index() {
        let state = AppState::new(fixture_sections());
        let panes = PaneAreas {
            sections: Rect::new(0, 0, 20, 10),
            messages: Rect::new(20, 0, 40, 10),
        };
        let event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 1,
            modifiers: event::KeyModifiers::NONE,
        };
        assert_eq!(
            action_from_mouse_event(event, &panes, 0, 0, &state),
            Some(Action::SelectSection(0))
        );
    }

    #[test]
    fn wheel_up_over_messages_maps_to_targeted_message_move() {
        let mut state = AppState::new(fixture_sections());
        state.apply(Action::FocusRight);
        let panes = PaneAreas {
            sections: Rect::new(0, 0, 20, 10),
            messages: Rect::new(20, 0, 40, 10),
        };
        let event = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 25,
            row: 3,
            modifiers: event::KeyModifiers::NONE,
        };
        assert_eq!(
            action_from_mouse_event(event, &panes, 0, 0, &state),
            Some(Action::MoveUpIn(FocusedPane::Messages))
        );
    }

    #[test]
    fn wheel_down_over_sections_targets_sections_even_when_messages_are_focused() {
        let mut state = AppState::new(fixture_sections());
        state.apply(Action::FocusRight);
        let panes = PaneAreas {
            sections: Rect::new(0, 0, 20, 10),
            messages: Rect::new(20, 0, 40, 10),
        };
        let event = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 1,
            row: 3,
            modifiers: event::KeyModifiers::NONE,
        };
        assert_eq!(
            action_from_mouse_event(event, &panes, 0, 0, &state),
            Some(Action::MoveDownIn(FocusedPane::Sections))
        );
    }

    #[test]
    fn click_inside_message_pane_below_last_item_only_focuses_messages() {
        let mut state = AppState::new(fixture_sections());
        state.apply(Action::SelectSection(1));
        let panes = PaneAreas {
            sections: Rect::new(0, 0, 20, 10),
            messages: Rect::new(20, 0, 40, 5),
        };
        let event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 25,
            row: 3,
            modifiers: event::KeyModifiers::NONE,
        };
        assert_eq!(
            action_from_mouse_event(event, &panes, 0, 0, &state),
            Some(Action::FocusRight)
        );
    }

    #[test]
    fn scrollbar_uses_selected_position_not_viewport_offset() {
        assert_eq!(scrollbar_position(Some(35), 5), 35);
        assert_eq!(scrollbar_position(None, 5), 5);
    }

    #[test]
    fn click_after_scroll_uses_message_offset_for_long_list() {
        let mut state = AppState::new(fixture_sections());
        state.apply(Action::SelectSection(3));
        let panes = PaneAreas {
            sections: Rect::new(0, 0, 20, 10),
            messages: Rect::new(20, 0, 40, 6),
        };
        let event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 25,
            row: 2,
            modifiers: event::KeyModifiers::NONE,
        };
        assert_eq!(
            action_from_mouse_event(event, &panes, 0, 20, &state),
            Some(Action::SelectMessage(21))
        );
    }

    #[test]
    fn clicks_on_border_and_after_visible_window_do_not_select_items() {
        let state = AppState::new(fixture_sections());
        let panes = PaneAreas {
            sections: Rect::new(0, 0, 20, 5),
            messages: Rect::new(20, 0, 40, 5),
        };

        let top_border = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: event::KeyModifiers::NONE,
        };
        assert_eq!(
            action_from_mouse_event(top_border, &panes, 0, 0, &state),
            Some(Action::FocusLeft)
        );

        let bottom_border = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 4,
            modifiers: event::KeyModifiers::NONE,
        };
        assert_eq!(
            action_from_mouse_event(bottom_border, &panes, 0, 0, &state),
            Some(Action::FocusLeft)
        );

        let past_visible_window = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 25,
            row: 2,
            modifiers: event::KeyModifiers::NONE,
        };
        assert_eq!(
            action_from_mouse_event(past_visible_window, &panes, 0, 99, &state),
            Some(Action::FocusRight)
        );
    }
    #[test]
    fn composer_content_height_empty_text_is_one_row() {
        assert_eq!(composer_content_height("", 20), 1);
    }

    #[test]
    fn composer_content_height_caps_twelve_newline_lines_at_ten() {
        assert_eq!(composer_content_height(&("x\n".repeat(11) + "x"), 20), 10);
    }

    #[test]
    fn composer_ui_areas_for_eighty_by_twenty_with_five_rows_give_expected_heights() {
        let areas = ui_areas(Rect::new(0, 0, 80, 20), 5);

        assert_eq!(areas.composer, Rect::new(0, 13, 80, 7));
        assert_eq!(areas.panes.sections.height, 13);
        assert_eq!(areas.panes.messages.height, 13);
    }

    #[test]
    fn composer_ui_areas_for_two_rows_use_full_frame_without_upper_panes() {
        let areas = ui_areas(Rect::new(0, 0, 20, 2), 1);

        assert_eq!(areas.composer, Rect::new(0, 0, 20, 2));
        assert_eq!(areas.panes.sections.height, 0);
        assert_eq!(areas.panes.messages.height, 0);
    }

    #[test]
    fn composer_viewport_top_tracks_cursor_in_last_ten_visual_lines() {
        let text = "x\n".repeat(11) + "x";
        assert_eq!(composer_viewport_top(&text, 20, text.len(), 0), 2);
    }

    #[test]
    fn composer_draw_syncs_scroll_and_submit_resets_it() {
        let backend = TestBackend::new(20, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(fixture_sections());
        let mut view = ViewState::default();
        let text = "x\n".repeat(11) + "x";

        state.apply(Action::InsertText(text));

        terminal
            .draw(|frame| {
                let _ = draw_ui(frame, &mut state, &mut view);
            })
            .unwrap();

        assert_eq!(state.composer_scroll(), 2);

        state.apply(Action::SubmitComposer);

        assert_eq!(state.composer_scroll(), 0);
    }

    #[test]
    fn composer_unicode_wide_char_wrap_matches_terminal_cells() {
        assert_eq!(composer_content_height("a好", 2), 2);
        assert_eq!(composer_cursor_row("a好", 2, "a".len()), 0);
        assert_eq!(composer_cursor_row("a好", 2, "a好".len()), 1);
    }

    #[test]
    fn composer_unicode_combining_mark_does_not_add_width() {
        let text = "a\u{0301}b";

        assert_eq!(composer_content_height(text, 1), 2);
        assert_eq!(composer_cursor_row(text, 1, "a".len()), 0);
        assert_eq!(composer_cursor_row(text, 1, "a\u{0301}".len()), 0);
        assert_eq!(composer_cursor_row(text, 1, text.len()), 1);
    }

    #[test]
    fn composer_unicode_emoji_updates_viewport_by_display_width() {
        let text = format!("{}a😀", "x\n".repeat(9));

        assert_eq!(composer_content_height(&text, 2), 10);
        assert_eq!(composer_viewport_top(&text, 2, text.len(), 0), 1);
    }

    #[test]
    fn composer_renders_below_upper_panes_after_five_lines() {
        let backend = TestBackend::new(20, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(fixture_sections());
        let mut view = ViewState::default();

        state.apply(Action::InsertText("a\nb\nc\nd\ne".into()));

        terminal
            .draw(|frame| {
                let _ = draw_ui(frame, &mut state, &mut view);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let border_row = (0..20)
            .map(|x| buffer[(x, 12)].symbol())
            .collect::<String>();
        let composer_row = (0..20)
            .map(|x| buffer[(x, 13)].symbol())
            .collect::<String>();

        assert!(border_row.contains("└") || border_row.contains("┘") || border_row.contains("─"));
        assert!(composer_row.contains("Composer"));
    }

    #[test]
    fn click_on_composer_focuses_composer() {
        let event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 18,
            modifiers: KeyModifiers::NONE,
        };

        assert_eq!(
            composer_action_from_mouse_event(event, Rect::new(0, 17, 20, 3)),
            Some(Action::FocusComposer)
        );
    }

    #[test]
    fn composer_renders_image_dimensions_in_attachment_label() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(vec![]);
        let mut view = ViewState::default();
        state.apply(Action::InsertImage {
            width: 1059,
            height: 200,
            rgba: vec![0; 4],
        });

        terminal
            .draw(|frame| {
                let _ = draw_ui(frame, &mut state, &mut view);
            })
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("[Image #0, 1059x200]"));
    }

    #[test]
    fn focused_composer_places_visible_cursor_at_draft_position() {
        let backend = TestBackend::new(20, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(vec![]);
        let mut view = ViewState::default();
        state.focused_pane = FocusedPane::Composer;
        state.apply(Action::InsertText("hi".into()));
        state.apply(Action::MoveCursorLeft);

        terminal
            .draw(|frame| {
                let _ = draw_ui(frame, &mut state, &mut view);
            })
            .unwrap();

        assert_eq!(terminal.get_cursor_position().unwrap(), (2, 8).into());
    }

    #[test]
    fn restoration_plan_pops_keyboard_enhancement_before_raw_mode() {
        assert_eq!(
            restoration_plan(SetupState {
                raw_mode_enabled: true,
                keyboard_enhancement_enabled: true,
                ..SetupState::default()
            }),
            vec![
                CleanupStep::PopKeyboardEnhancement,
                CleanupStep::DisableRawMode,
            ]
        );
    }
}
