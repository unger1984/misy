//! Persistent, cross-session prompt history for the terminal client.

use misy_core::MisyPaths;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::PathBuf,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const MAX_HISTORY_ENTRIES: usize = 100;
const LOCK_ATTEMPTS: usize = 10;
const LOCK_RETRY: Duration = Duration::from_millis(10);

#[derive(Clone, Debug)]
pub(super) struct PromptHistoryStore {
    path: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
struct StoredPrompt {
    submitted_at: u64,
    text: String,
}

impl PromptHistoryStore {
    pub(super) fn new(paths: &MisyPaths) -> Self {
        Self {
            path: paths.root().join("prompt-history.jsonl"),
        }
    }

    pub(super) fn load(&self) -> io::Result<Vec<String>> {
        let mut file = match OpenOptions::new().read(true).write(true).open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        lock_exclusive(&file)?;
        let result = read_entries(&mut file).map(|entries| {
            let skip = entries.len().saturating_sub(MAX_HISTORY_ENTRIES);
            entries
                .into_iter()
                .skip(skip)
                .map(|entry| entry.text)
                .collect()
        });
        let _ = file.unlock();
        result
    }

    pub(super) fn append(&self, text: &str) -> io::Result<()> {
        if text.trim().is_empty() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).append(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&self.path)?;
        ensure_private_permissions(&file)?;
        lock_exclusive(&file)?;
        let result = append_locked(&mut file, text);
        let _ = file.unlock();
        result
    }
}

fn append_locked(file: &mut File, text: &str) -> io::Result<()> {
    let mut entries = read_entries(file)?;
    if entries.last().is_some_and(|entry| entry.text == text) {
        return Ok(());
    }
    entries.push(StoredPrompt {
        submitted_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| io::Error::other(format!("system clock before Unix epoch: {error}")))?
            .as_secs(),
        text: text.to_owned(),
    });
    if entries.len() <= MAX_HISTORY_ENTRIES {
        file.seek(SeekFrom::End(0))?;
        return write_entry(file, entries.last().expect("new history entry must exist"));
    }
    let retained = entries.split_off(entries.len() - MAX_HISTORY_ENTRIES);
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    for entry in &retained {
        write_entry(file, entry)?;
    }
    file.flush()
}

fn read_entries(file: &mut File) -> io::Result<Vec<StoredPrompt>> {
    file.seek(SeekFrom::Start(0))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(contents
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

fn write_entry(file: &mut File, entry: &StoredPrompt) -> io::Result<()> {
    let mut line = serde_json::to_vec(entry)
        .map_err(|error| io::Error::other(format!("could not encode prompt history: {error}")))?;
    line.push(b'\n');
    file.write_all(&line)?;
    file.flush()
}

fn lock_exclusive(file: &File) -> io::Result<()> {
    for _ in 0..LOCK_ATTEMPTS {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(fs::TryLockError::WouldBlock) => thread::sleep(LOCK_RETRY),
            Err(error) => return Err(error.into()),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "could not lock prompt history after bounded retries",
    ))
}

#[cfg(unix)]
fn ensure_private_permissions(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if metadata.permissions().mode() & 0o777 != 0o600 {
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_permissions(_file: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_round_trips_multiline_and_ignores_malformed_rows() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let paths = MisyPaths::from_root(temporary.path());
        let store = PromptHistoryStore::new(&paths);
        fs::write(store.path.clone(), "not-json\n").expect("malformed fixture");

        store.append("first\nsecond").expect("append history");

        assert_eq!(store.load().expect("load history"), ["first\nsecond"]);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&store.path)
                .expect("history metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn history_keeps_the_latest_hundred_and_collapses_adjacent_duplicates() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let store = PromptHistoryStore::new(&MisyPaths::from_root(temporary.path()));
        for index in 0..105 {
            store
                .append(&format!("prompt-{index}"))
                .expect("append history");
        }
        store.append("prompt-104").expect("append duplicate");

        let history = store.load().expect("load history");
        assert_eq!(history.len(), 100);
        assert_eq!(history.first().map(String::as_str), Some("prompt-5"));
        assert_eq!(history.last().map(String::as_str), Some("prompt-104"));
    }

    #[test]
    fn independent_tabs_append_without_corrupting_the_history() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let store = PromptHistoryStore::new(&MisyPaths::from_root(temporary.path()));
        let first = store.clone();
        let second = store.clone();
        let first_write = thread::spawn(move || first.append("from first tab"));
        let second_write = thread::spawn(move || second.append("from second tab"));

        first_write
            .join()
            .expect("first tab thread")
            .expect("first append");
        second_write
            .join()
            .expect("second tab thread")
            .expect("second append");
        let history = store.load().expect("load history");
        assert_eq!(history.len(), 2);
        assert!(history.iter().any(|entry| entry == "from first tab"));
        assert!(history.iter().any(|entry| entry == "from second tab"));
    }
}
