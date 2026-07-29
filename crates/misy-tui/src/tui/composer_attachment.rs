//! Atomic image tokens embedded in one composer draft.

use misy_core::{ImageAttachment, MAX_SUBMISSION_IMAGES};
use std::ops::Range;

#[derive(Clone, Debug, Eq, PartialEq)]
struct AttachedImage {
    image: ImageAttachment,
    token: Range<usize>,
}

/// Image-bearing submission captured from the composer without clearing it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ComposerDraft {
    pub(super) text: String,
    pub(super) images: Vec<ImageAttachment>,
}

/// Image tokens and their validated payloads for one editable draft.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ComposerAttachments {
    images: Vec<AttachedImage>,
}

impl ComposerAttachments {
    pub(super) fn len(&self) -> usize {
        self.images.len()
    }

    pub(super) fn insert(
        &mut self,
        text: &mut String,
        cursor: &mut usize,
        image: ImageAttachment,
    ) -> Result<(), &'static str> {
        if self.images.len() >= MAX_SUBMISSION_IMAGES {
            return Err("a prompt can contain at most 4 images");
        }
        let label = format!("[Image #{}]", self.images.len() + 1);
        let insertion = format!("{label} ");
        self.shift_for_insert(*cursor, insertion.len());
        text.insert_str(*cursor, &insertion);
        let start = *cursor;
        *cursor += insertion.len();
        self.images.push(AttachedImage {
            image,
            token: start..*cursor,
        });
        Ok(())
    }

    pub(super) fn shift_for_insert(&mut self, at: usize, bytes: usize) {
        for attached in &mut self.images {
            if attached.token.start >= at {
                attached.token.start += bytes;
                attached.token.end += bytes;
            }
        }
    }

    pub(super) fn remove_before(&mut self, text: &mut String, cursor: &mut usize) -> bool {
        let Some(index) = self
            .images
            .iter()
            .position(|attached| attached.token.start < *cursor && *cursor <= attached.token.end)
        else {
            return false;
        };
        let token = self.images[index].token.clone();
        self.remove_token(text, cursor, index, token);
        true
    }

    pub(super) fn remove_at(&mut self, text: &mut String, cursor: &mut usize) -> bool {
        let Some(index) = self
            .images
            .iter()
            .position(|attached| attached.token.start <= *cursor && *cursor < attached.token.end)
        else {
            return false;
        };
        let token = self.images[index].token.clone();
        self.remove_token(text, cursor, index, token);
        true
    }

    pub(super) fn shift_for_remove(&mut self, removed: Range<usize>) {
        let bytes = removed.end.saturating_sub(removed.start);
        for attached in &mut self.images {
            if attached.token.start >= removed.end {
                attached.token.start -= bytes;
                attached.token.end -= bytes;
            }
        }
    }

    pub(super) fn previous_cursor(&self, text: &str, cursor: usize) -> usize {
        if let Some(attached) = self
            .images
            .iter()
            .find(|attached| attached.token.end == cursor)
        {
            return attached.token.start;
        }
        previous_boundary(text, cursor)
    }

    pub(super) fn next_cursor(&self, text: &str, cursor: usize) -> usize {
        if let Some(attached) = self
            .images
            .iter()
            .find(|attached| attached.token.start == cursor)
        {
            return attached.token.end;
        }
        next_boundary(text, cursor)
    }

    pub(super) fn snap_cursor(&self, cursor: usize) -> usize {
        let Some(attached) = self
            .images
            .iter()
            .find(|attached| attached.token.start < cursor && cursor < attached.token.end)
        else {
            return cursor;
        };
        let midpoint = attached.token.start + attached.token.len() / 2;
        if cursor <= midpoint {
            attached.token.start
        } else {
            attached.token.end
        }
    }

    pub(super) fn draft(&self, text: &str) -> ComposerDraft {
        ComposerDraft {
            text: self.text_without_tokens(text),
            images: self
                .images
                .iter()
                .map(|attached| attached.image.clone())
                .collect(),
        }
    }

    pub(super) fn history_text(&self, text: &str) -> String {
        self.text_without_tokens(text).trim().to_owned()
    }

    fn text_without_tokens(&self, text: &str) -> String {
        let mut plain = text.to_owned();
        for attached in self.images.iter().rev() {
            plain.replace_range(attached.token.clone(), "");
        }
        plain
    }

    pub(super) fn clear(&mut self) {
        self.images.clear();
    }

    fn remove_token(
        &mut self,
        text: &mut String,
        cursor: &mut usize,
        index: usize,
        token: Range<usize>,
    ) {
        text.drain(token.clone());
        *cursor = token.start;
        self.images.remove(index);
        self.shift_for_remove(token);
        self.renumber(text);
    }

    fn renumber(&mut self, text: &mut String) {
        for (index, attached) in self.images.iter().enumerate() {
            let label = format!("[Image #{}]", index + 1);
            let label_end = attached.token.end.saturating_sub(1);
            text.replace_range(attached.token.start..label_end, &label);
        }
    }
}

fn previous_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map(|character| cursor + character.len_utf8())
        .unwrap_or(text.len())
}
