//! Unicode display-width helpers shared by bounded terminal layouts.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) fn text_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub(super) fn truncate_to_width(text: &str, width: usize) -> String {
    if text_width(text) <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let content_width = width.saturating_sub(1);
    let mut rendered = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > content_width {
            break;
        }
        rendered.push(character);
        used += character_width;
    }
    rendered.push('…');
    rendered
}
