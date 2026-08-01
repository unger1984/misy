//! Bounded blocking filesystem helpers used by local tools.

use std::{fs, io::Read};

pub(super) const MAX_TEXT_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 10_000;

pub(super) fn read_file_contents(path: &str) -> Result<String, String> {
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
    let truncated = bytes.len() > MAX_TEXT_FILE_BYTES;
    bytes.truncate(MAX_TEXT_FILE_BYTES);
    let mut content = match String::from_utf8(bytes) {
        Ok(content) => content,
        Err(error) if truncated && error.utf8_error().error_len().is_none() => {
            let valid_up_to = error.utf8_error().valid_up_to();
            let mut bytes = error.into_bytes();
            bytes.truncate(valid_up_to);
            String::from_utf8_lossy(&bytes).into_owned()
        }
        Err(_) => return Err(format!("could not read {path}: file is not valid UTF-8")),
    };
    if truncated {
        content.push_str(&format!(
            "\n[... truncated: showing the first {MAX_TEXT_FILE_BYTES} of {} bytes. \
             Use exec_command, e.g. `sed -n` or `tail`, to read the rest ...]",
            metadata.len()
        ));
    }
    Ok(content)
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
