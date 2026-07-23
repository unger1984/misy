use std::io;

use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Scrollbar, ScrollbarOrientation,
        ScrollbarState,
    },
};

use crate::app::{Action, AppState, FocusedPane, fixture_sections};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SetupState {
    raw_mode_enabled: bool,
    alternate_screen_entered: bool,
    mouse_capture_enabled: bool,
    cursor_hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CleanupStep {
    ShowCursor,
    DisableMouseCapture,
    LeaveAlternateScreen,
    DisableRawMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PaneAreas {
    sections: Rect,
    messages: Rect,
}

#[derive(Debug, Default)]
struct ViewState {
    sections: ListState,
    messages: ListState,
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

    if let Err(error) = execute!(stdout, Hide) {
        return Err(merge_cleanup_error(error, restore_terminal(setup)));
    }
    setup.cursor_hidden = true;

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
    let mut last_panes = PaneAreas::default();

    let runtime_result = (|| -> io::Result<()> {
        while state.running {
            terminal.draw(|frame| {
                last_panes = draw_ui(frame, &state, &mut view);
            })?;

            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if let Some(action) = action_from_key_event(key) {
                        state.apply(action);
                    }
                }
                Event::Mouse(mouse) => {
                    if let Some(action) = action_from_mouse_event(
                        mouse,
                        &last_panes,
                        view.sections.offset(),
                        view.messages.offset(),
                        &state,
                    ) {
                        state.apply(action);
                    }
                }
                _ => {}
            }
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
    let mut steps = Vec::with_capacity(4);
    if state.cursor_hidden {
        steps.push(CleanupStep::ShowCursor);
    }
    if state.mouse_capture_enabled {
        steps.push(CleanupStep::DisableMouseCapture);
    }
    if state.alternate_screen_entered {
        steps.push(CleanupStep::LeaveAlternateScreen);
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

fn pane_areas(area: Rect) -> PaneAreas {
    let [sections, messages] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)]).areas(area);
    PaneAreas { sections, messages }
}

fn draw_ui(frame: &mut Frame, state: &AppState, view: &mut ViewState) -> PaneAreas {
    let frame_area = frame.area();
    frame.render_widget(Clear, frame_area);
    let panes = pane_areas(frame_area);

    view.sections
        .select((!state.sections.is_empty()).then_some(state.selected_section));
    view.messages
        .select((!state.current_messages().is_empty()).then_some(state.selected_message));

    let section_content_width = usize::from(panes.sections.width.saturating_sub(2));
    let section_viewport_height = usize::from(panes.sections.height.saturating_sub(2));
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
        panes.sections.inner(Margin {
            vertical: 1,
            horizontal: 1,
        }),
    );
    frame.render_stateful_widget(
        List::new(section_items)
            .block(sections_block)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        panes.sections,
        &mut view.sections,
    );

    render_scrollbar(
        frame,
        panes.sections,
        state.sections.len(),
        scrollbar_position(
            (!state.sections.is_empty()).then_some(state.selected_section),
            view.sections.offset(),
        ),
    );

    let message_content_width = usize::from(panes.messages.width.saturating_sub(2));
    let message_viewport_height = usize::from(panes.messages.height.saturating_sub(2));
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
                .map(|message| ListItem::new(padded_cell(message.as_str(), message_content_width)))
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
        panes.messages.inner(Margin {
            vertical: 1,
            horizontal: 1,
        }),
    );
    frame.render_stateful_widget(
        List::new(message_items)
            .block(messages_block)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        panes.messages,
        &mut view.messages,
    );

    render_scrollbar(
        frame,
        panes.messages,
        state.current_messages().len(),
        scrollbar_position(
            (!state.current_messages().is_empty()).then_some(state.selected_message),
            view.messages.offset(),
        ),
    );

    panes
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
        let state = AppState::new(vec![]);
        let mut view = ViewState::default();

        terminal
            .draw(|frame| {
                let _ = draw_ui(frame, &state, &mut view);
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
        let state = AppState::new(vec![Section {
            title: "Empty".into(),
            messages: vec![],
        }]);
        let mut view = ViewState::default();

        terminal
            .draw(|frame| {
                let _ = draw_ui(frame, &state, &mut view);
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
            }),
            vec![
                CleanupStep::ShowCursor,
                CleanupStep::DisableMouseCapture,
                CleanupStep::LeaveAlternateScreen,
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
}
