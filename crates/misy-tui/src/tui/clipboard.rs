//! Clipboard boundaries for explicit paste and terminal-owned copy feedback.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{error::Error, fmt, io, io::Write};

const MAX_OSC52_TEXT_BYTES: usize = 196_602;

/// Clipboard failure surfaced in the terminal transcript without ending the session.
#[derive(Debug)]
pub(super) enum ClipboardError {
    Access(arboard::Error),
    InvalidImageDimensions,
}

impl fmt::Display for ClipboardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Access(error) => write!(formatter, "could not access the clipboard: {error}"),
            Self::InvalidImageDimensions => {
                formatter.write_str("clipboard image dimensions are too large")
            }
        }
    }
}

impl Error for ClipboardError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Access(error) => Some(error),
            Self::InvalidImageDimensions => None,
        }
    }
}

/// One payload chosen by an explicit paste chord.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum ClipboardPaste {
    Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Text(String),
}

/// Clipboard boundary used by the terminal event adapter.
pub(super) trait Clipboard {
    fn paste(&mut self) -> Result<ClipboardPaste, ClipboardError>;
}

/// Native clipboard implementation backed by the host window system.
#[derive(Default)]
pub(super) struct SystemClipboard {
    clipboard: Option<arboard::Clipboard>,
}

impl SystemClipboard {
    fn clipboard(&mut self) -> Result<&mut arboard::Clipboard, ClipboardError> {
        if self.clipboard.is_none() {
            self.clipboard = Some(arboard::Clipboard::new().map_err(ClipboardError::Access)?);
        }
        match self.clipboard.as_mut() {
            Some(clipboard) => Ok(clipboard),
            None => Err(ClipboardError::Access(
                arboard::Error::ClipboardNotSupported,
            )),
        }
    }
}

impl Clipboard for SystemClipboard {
    fn paste(&mut self) -> Result<ClipboardPaste, ClipboardError> {
        let clipboard = self.clipboard()?;
        match clipboard.get_image() {
            Ok(image) => Ok(ClipboardPaste::Image {
                width: u32::try_from(image.width)
                    .map_err(|_| ClipboardError::InvalidImageDimensions)?,
                height: u32::try_from(image.height)
                    .map_err(|_| ClipboardError::InvalidImageDimensions)?,
                rgba: image.bytes.into_owned(),
            }),
            Err(arboard::Error::ContentNotAvailable) => clipboard
                .get_text()
                .map(ClipboardPaste::Text)
                .map_err(ClipboardError::Access),
            Err(error) => Err(ClipboardError::Access(error)),
        }
    }
}

/// Test double for hosts where the system clipboard is unavailable, so the
/// paste-failure path can be exercised without a real window system.
#[cfg(test)]
pub(super) struct FailingClipboard;

#[cfg(test)]
impl Clipboard for FailingClipboard {
    fn paste(&mut self) -> Result<ClipboardPaste, ClipboardError> {
        Err(ClipboardError::Access(
            arboard::Error::ClipboardNotSupported,
        ))
    }
}

pub(super) fn write_osc52_copy(writer: &mut impl Write, text: &str) -> Result<(), io::Error> {
    if text.len() > MAX_OSC52_TEXT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "selection is too large for terminal clipboard transfer",
        ));
    }
    write!(writer, "\x1b]52;c;{}\x07", STANDARD.encode(text))?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_uses_the_terminal_clipboard_channel() {
        let mut output = Vec::new();
        write_osc52_copy(&mut output, "hello").expect("encode clipboard text");
        assert_eq!(output, b"\x1b]52;c;aGVsbG8=\x07");
    }

    #[test]
    fn osc52_rejects_payloads_above_the_host_limit() {
        let mut output = Vec::new();
        let error = write_osc52_copy(&mut output, &"x".repeat(MAX_OSC52_TEXT_BYTES + 1))
            .expect_err("reject oversized clipboard text");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
