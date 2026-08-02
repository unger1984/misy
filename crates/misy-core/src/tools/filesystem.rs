//! Bounded blocking filesystem helpers used by local tools.

use std::{fs, io::Read, ops::Range};

pub(super) const MAX_TEXT_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 10_000;

pub(super) fn read_file_contents(path: &str) -> Result<String, String> {
    let projection = load_bounded_utf8(path)?;
    Ok(format_unpaged(&projection))
}

pub(super) fn read_file_page(path: &str, offset: u64, limit: usize) -> Result<String, String> {
    let projection = load_bounded_utf8(path)?;
    Ok(format_page(&projection, offset, limit))
}

struct BoundedText {
    content: String,
    read_boundary_reached: bool,
    physical_bytes: u64,
}

fn load_bounded_utf8(path: &str) -> Result<BoundedText, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("could not read {path}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!(
            "read_file supports regular files only: {path} is not a regular file"
        ));
    }
    let file = fs::File::open(path).map_err(|error| format!("could not read {path}: {error}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_TEXT_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read {path}: {error}"))?;
    let read_boundary_reached = bytes.len() > MAX_TEXT_FILE_BYTES;
    bytes.truncate(MAX_TEXT_FILE_BYTES);
    let content = match String::from_utf8(bytes) {
        Ok(content) => content,
        Err(error) if read_boundary_reached && error.utf8_error().error_len().is_none() => {
            let valid_up_to = error.utf8_error().valid_up_to();
            let mut bytes = error.into_bytes();
            bytes.truncate(valid_up_to);
            String::from_utf8_lossy(&bytes).into_owned()
        }
        Err(_) => return Err(format!("could not read {path}: file is not valid UTF-8")),
    };
    Ok(BoundedText {
        content,
        read_boundary_reached,
        physical_bytes: metadata.len(),
    })
}

fn format_unpaged(projection: &BoundedText) -> String {
    let mut content = projection.content.clone();
    if projection.read_boundary_reached {
        content.push_str(&format!(
            "\n[... truncated: showing the first {MAX_TEXT_FILE_BYTES} of {} bytes. \
             Use exec_command, e.g. `sed -n` or `tail`, to read the rest ...]",
            projection.physical_bytes
        ));
    }
    content
}

fn format_page(projection: &BoundedText, offset: u64, limit: usize) -> String {
    let spans = line_spans(&projection.content);
    let start = offset
        .checked_sub(1)
        .and_then(|index| usize::try_from(index).ok());
    let selected = start
        .and_then(|index| spans.get(index..))
        .map_or(&[][..], |remaining| {
            &remaining[..remaining.len().min(limit)]
        });
    let range = selected.last().map_or_else(
        || "empty".to_owned(),
        |_| format!("{offset}-{}", offset + selected.len() as u64 - 1),
    );
    let mut page = format!("[read_file page offset={offset} range={range}]\n");
    for span in selected {
        page.push_str(&projection.content[span.clone()]);
    }
    if !page.ends_with('\n') {
        page.push('\n');
    }
    let next_index = start.and_then(|index| index.checked_add(selected.len()));
    let terminal = match next_index.filter(|index| *index < spans.len()) {
        Some(_) => format!("next_offset={}", offset + selected.len() as u64),
        None if projection.read_boundary_reached => "read_boundary_reached".to_owned(),
        None => "end_of_file".to_owned(),
    };
    page.push_str(&format!("[{terminal}]"));
    page
}

fn line_spans(content: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            spans.push(start..index + 1);
            start = index + 1;
        }
    }
    if start < content.len() {
        spans.push(start..content.len());
    }
    spans
}

pub(super) fn read_complete_utf8_file(path: &str) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("could not read {path}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!(
            "StrReplaceFile supports regular files only: {path} is not a regular file"
        ));
    }
    if metadata.len() > MAX_TEXT_FILE_BYTES as u64 {
        return Err(format!(
            "could not read {path}: file exceeds the {MAX_TEXT_FILE_BYTES}-byte limit"
        ));
    }
    let file = fs::File::open(path).map_err(|error| format!("could not read {path}: {error}"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_TEXT_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read {path}: {error}"))?;
    if bytes.len() > MAX_TEXT_FILE_BYTES {
        return Err(format!(
            "could not read {path}: file exceeds the {MAX_TEXT_FILE_BYTES}-byte limit"
        ));
    }
    String::from_utf8(bytes).map_err(|_| format!("could not read {path}: file is not valid UTF-8"))
}

pub(super) fn list_directory_entries(path: &str) -> Result<Vec<String>, String> {
    let entries = fs::read_dir(path).map_err(|error| format!("could not list {path}: {error}"))?;
    let mut names = Vec::new();
    let mut overflow = 0_usize;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("could not inspect an entry in {path}: {error}"))?;
        if names.len() < MAX_DIRECTORY_ENTRIES {
            names.push(entry.file_name().to_string_lossy().into_owned());
        } else {
            overflow += 1;
        }
    }
    names.sort_unstable();
    if overflow > 0 {
        names.push(format!("... and {overflow} more"));
    }
    Ok(names)
}
