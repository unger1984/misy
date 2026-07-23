# TUI Navigation Bootstrap Design

## Summary

The next small, user-visible step for `misy` is a two-pane TUI navigation shell that supports both keyboard and mouse interaction.

The left pane shows sections. The right pane shows messages for the selected section. The goal is not to build a chat yet, but to establish a real interaction loop that can be launched and manually exercised.

## Goals

- Make the current bootstrap TUI meaningfully interactive.
- Support both keyboard and mouse navigation in the first version.
- Keep the scope small enough for one implementation pass.
- Preserve a clean path toward a future session-oriented agent UI.

## Non-Goals

This step does not include:

- text input;
- command palette;
- real agent/runtime integration;
- persistence;
- async loading;
- message detail preview;
- drag interactions;
- advanced hotkeys beyond the basic navigation set.

## Screen Layout

The application shows one main screen with two panes:

- left pane: sections;
- right pane: messages for the selected section.

This is a fixed two-column layout for the first version.

## State Model

`AppState` should grow from a simple running flag into a small explicit navigation state.

Required state:

- `sections: Vec<Section>`
- `selected_section: usize`
- `selected_message: usize`
- `focused_pane: FocusedPane`
- `running: bool`

`Section` contains:

- `title: String`
- `messages: Vec<String>`

`FocusedPane` contains:

- `Sections`
- `Messages`

This is intentionally minimal. Stable IDs, timestamps, and metadata are unnecessary for this step.

## Interaction Model

### Keyboard

Required keys:

- `Up` / `Down`: move selection inside the focused pane;
- `Left` / `Right`: move focus between panes;
- `Tab`: cycle focus between panes;
- `Enter`: when the left pane is focused, confirm the selected section and move focus to the right pane;
- `q`: quit.

Behavior rules:

- selection does not wrap at list boundaries;
- `Left` / `Right` only change focus, not item selection;
- `Enter` on the right pane does nothing in this version.

### Mouse

Required mouse support:

- click on a pane gives it focus;
- click on an item focuses the pane and selects the clicked item;
- mouse wheel moves selection in the pane under the cursor.

If cursor-local wheel routing proves awkward in the first pass, focus-based wheel routing is acceptable as a fallback, but cursor-local behavior is preferred.

## Pane Behavior

### Section changes

When the selected section changes:

- the right pane updates to show that section's messages;
- `selected_message` resets to the first message if messages exist;
- if the section has no messages, the right pane remains empty but can still receive focus.

### Empty states

The UI must handle empty content explicitly:

- no sections: left pane shows `No sections`;
- no messages in the selected section: right pane shows `No messages`.

## Fixture Data

The first version should use in-memory fixture data so the UI is testable immediately on launch.

Fixture requirements:

- around 4 sections;
- one empty section;
- one short section;
- one medium section;
- one long section with roughly 30-50 messages to exercise scrolling.

The long list exists specifically to validate scrolling behavior and selection visibility in a constrained terminal viewport.

## Rendering and Event Handling

The design should preserve a simple but correct rule:

- rendering reads state;
- input transforms state.

The input path should parse `crossterm::event::Event`, map it into a small action model or equivalent state transition path, and then update `AppState` outside rendering.

The implementation should avoid pushing navigation logic into `draw_ui`.

## UX Edge Cases

The first implementation must behave safely in these cases:

- moving above the first item leaves selection at the first item;
- moving below the last item leaves selection at the last item;
- clicking or scrolling in an empty pane does not break state;
- switching to an empty section keeps the right pane valid;
- long-list selection remains usable when the selected item moves beyond the initially visible area.

## Manual Verification

Done means the feature can be manually exercised in the terminal.

Required smoke checks:

1. launch the TUI;
2. move through list items with `Up` / `Down`;
3. switch focus with `Tab`, `Left`, and `Right`;
4. click items in both panes;
5. use the mouse wheel in both panes;
6. switch between empty and non-empty sections;
7. verify the long list scroll path;
8. quit with `q`.

## Why This Step

This step is small, visible, and structural.

It does not pretend to implement agent behavior yet. Instead, it creates a real navigation shell that can later absorb:

- a bottom input line;
- message detail preview;
- session-backed data instead of fixtures.

That makes it a good next step: small enough to finish, but not throwaway.
