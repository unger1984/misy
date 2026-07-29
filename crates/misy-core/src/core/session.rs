//! Append-only persistence for core-owned conversation sessions.

use super::HistoryEntry;
use crate::{MessageRole, ModelRef, TodoItem};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const SESSION_VERSION: u32 = 1;
static NEXT_SESSION_SUFFIX: AtomicU64 = AtomicU64::new(1);

/// Compact metadata displayed by session pickers and CLI selectors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSummary {
    /// Stable identifier accepted by resume operations.
    pub id: String,
    /// Unix timestamp at which the session was first persisted.
    pub created_at: u64,
    /// Unix timestamp of the latest filesystem modification.
    pub modified_at: u64,
    /// Canonical working directory that owns the session.
    pub cwd: PathBuf,
    /// First user message, truncated for compact display.
    pub preview: String,
    /// Model associated with the session summary, when known.
    pub model: Option<ModelRef>,
}

/// Result of replacing the current conversation with a persisted session.
#[derive(Clone, Debug, PartialEq)]
pub struct ResumeOutcome {
    /// Metadata for the resumed session.
    pub session: SessionSummary,
    /// Number of canonical history entries restored into the core.
    pub history_len: usize,
    /// Canonical entries used by clients to rebuild their transcript projection.
    pub history: Vec<HistoryEntry>,
    /// Root checklist restored from the same append-only session.
    pub todos: Vec<TodoItem>,
    /// Non-fatal warning when the saved model cannot be selected.
    pub model_warning: Option<String>,
}

/// Failures produced by the versioned session store.
#[derive(Debug)]
pub enum SessionError {
    /// Filesystem access failed.
    Io(std::io::Error),
    /// A JSON line could not be encoded or a required header could not be decoded.
    Json(serde_json::Error),
    /// No saved session matched the requested id or prefix.
    NotFound(String),
    /// More than one saved session matched the requested prefix.
    Ambiguous(String),
    /// The file uses a schema revision this build cannot read.
    UnsupportedVersion(u32),
    /// A core session mutex was poisoned by a prior panic.
    StatePoisoned,
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "storage access failed: {error}"),
            Self::Json(error) => write!(formatter, "invalid session data: {error}"),
            Self::NotFound(id) => write!(formatter, "session `{id}` was not found"),
            Self::Ambiguous(id) => write!(formatter, "session prefix `{id}` is ambiguous"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported session version {version}")
            }
            Self::StatePoisoned => formatter.write_str("session state is unavailable"),
        }
    }
}

impl Error for SessionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for SessionError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for SessionError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SessionHeader {
    version: u32,
    id: String,
    parent_session: Option<String>,
    cwd: PathBuf,
    created_at: u64,
    cli_version: String,
    model: Option<ModelRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SessionRecordLine {
    ordinal: u64,
    timestamp: u64,
    record: SessionRecord,
}

#[derive(Deserialize)]
struct SessionRecordEnvelope {
    ordinal: u64,
    #[serde(rename = "timestamp")]
    _timestamp: u64,
    record: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SessionRecord {
    HistoryEntry { entry: HistoryEntry },
    ModelChange { model: ModelRef },
    TodoListUpdate { todos: Vec<TodoItem> },
}

struct CurrentSession {
    id: String,
    path: PathBuf,
    next_ordinal: u64,
}

pub(crate) struct SessionState {
    directory: PathBuf,
    cwd: PathBuf,
    current: Option<CurrentSession>,
    poisoned: bool,
}

pub(super) struct LoadedSession {
    pub(super) summary: SessionSummary,
    pub(super) history: Vec<HistoryEntry>,
    pub(super) todos: Vec<TodoItem>,
    pub(super) next_ordinal: u64,
    pub(super) path: PathBuf,
}

impl SessionState {
    pub(super) fn new(directory: PathBuf) -> Result<Self, SessionError> {
        let cwd = std::env::current_dir()?.canonicalize()?;
        Ok(Self {
            directory,
            cwd,
            current: None,
            poisoned: false,
        })
    }

    pub(super) fn current_id(&self) -> Option<String> {
        self.current.as_ref().map(|session| session.id.clone())
    }

    pub(super) fn detach(&mut self) {
        self.current = None;
        self.poisoned = false;
    }

    pub(super) fn append_history(
        &mut self,
        entry: HistoryEntry,
        model: Option<ModelRef>,
    ) -> Result<(), SessionError> {
        self.append(SessionRecord::HistoryEntry { entry }, model)
    }

    pub(super) fn append_model(&mut self, model: ModelRef) -> Result<(), SessionError> {
        if self.current.is_none() {
            return Ok(());
        }
        self.append(
            SessionRecord::ModelChange {
                model: model.clone(),
            },
            Some(model),
        )
    }

    pub(super) fn append_todos(
        &mut self,
        todos: Vec<TodoItem>,
        model: Option<ModelRef>,
    ) -> Result<(), SessionError> {
        self.append(SessionRecord::TodoListUpdate { todos }, model)
    }

    fn append(
        &mut self,
        record: SessionRecord,
        model: Option<ModelRef>,
    ) -> Result<(), SessionError> {
        if self.poisoned {
            return Ok(());
        }
        if self.current.is_none() {
            match self.create(model) {
                Ok(current) => self.current = Some(current),
                Err(error) => {
                    self.poisoned = true;
                    return Err(error);
                }
            }
        }
        let current = self.current.as_mut().ok_or(SessionError::StatePoisoned)?;
        let line = SessionRecordLine {
            ordinal: current.next_ordinal,
            timestamp: unix_time(),
            record,
        };
        match append_json_line(&current.path, &line) {
            Ok(()) => {
                current.next_ordinal += 1;
                Ok(())
            }
            Err(error) => {
                self.poisoned = true;
                Err(error)
            }
        }
    }

    fn create(&self, model: Option<ModelRef>) -> Result<CurrentSession, SessionError> {
        fs::create_dir_all(&self.directory)?;
        for _ in 0..100 {
            let id = session_id();
            let path = self.directory.join(format!("{id}.jsonl"));
            match create_private_file(&path) {
                Ok(mut file) => {
                    let header = SessionHeader {
                        version: SESSION_VERSION,
                        id: id.clone(),
                        parent_session: None,
                        cwd: self.cwd.clone(),
                        created_at: unix_time(),
                        cli_version: env!("CARGO_PKG_VERSION").to_owned(),
                        model,
                    };
                    serde_json::to_writer(&mut file, &header)?;
                    file.write_all(b"\n")?;
                    file.flush()?;
                    return Ok(CurrentSession {
                        id,
                        path,
                        next_ordinal: 1,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(SessionError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a unique session id",
        )))
    }

    pub(super) fn list_current_cwd(&self) -> Result<Vec<SessionSummary>, SessionError> {
        let mut summaries = self.list_all()?;
        summaries.retain(|summary| canonical_path(&summary.cwd) == self.cwd);
        summaries.sort_by(|left, right| {
            right
                .modified_at
                .cmp(&left.modified_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(summaries)
    }

    fn list_all(&self) -> Result<Vec<SessionSummary>, SessionError> {
        if !self.directory.exists() {
            return Ok(Vec::new());
        }
        let mut summaries = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            match summarize_path(&path) {
                Ok(summary) => summaries.push(summary),
                Err(SessionError::UnsupportedVersion(version)) => {
                    return Err(SessionError::UnsupportedVersion(version));
                }
                Err(_) => {}
            }
        }
        Ok(summaries)
    }

    pub(super) fn load(&self, requested: &str) -> Result<LoadedSession, SessionError> {
        let path = resolve_path(&self.directory, requested)?;
        load_path(&path)
    }

    pub(super) fn attach(&mut self, loaded: &LoadedSession) {
        self.current = Some(CurrentSession {
            id: loaded.summary.id.clone(),
            path: loaded.path.clone(),
            next_ordinal: loaded.next_ordinal,
        });
        self.poisoned = false;
    }
}

fn load_path(path: &Path) -> Result<LoadedSession, SessionError> {
    let file = File::open(path)?;
    let mut lines = BufReader::new(file).lines();
    let header = parse_header(&mut lines)?;
    let mut history = Vec::new();
    let mut todos = Vec::new();
    let mut model = header.model.clone();
    let mut next_ordinal = 1;
    for line in lines.map_while(Result::ok) {
        let Ok(envelope) = serde_json::from_str::<SessionRecordEnvelope>(&line) else {
            continue;
        };
        next_ordinal = next_ordinal.max(envelope.ordinal.saturating_add(1));
        let Ok(record) = serde_json::from_value::<SessionRecord>(envelope.record) else {
            continue;
        };
        match record {
            SessionRecord::HistoryEntry { entry } => history.push(entry),
            SessionRecord::ModelChange { model: changed } => model = Some(changed),
            SessionRecord::TodoListUpdate { todos: updated } => todos = updated,
        }
    }
    let preview = history
        .iter()
        .find(|entry| entry.message.role == MessageRole::User)
        .map(|entry| truncate_preview(&entry.message.content))
        .unwrap_or_else(|| "Untitled session".to_owned());
    let modified_at = modified_time(path, header.created_at)?;
    let summary = SessionSummary {
        id: header.id,
        created_at: header.created_at,
        modified_at,
        cwd: header.cwd,
        preview,
        model,
    };
    Ok(LoadedSession {
        summary,
        history,
        todos,
        next_ordinal,
        path: path.to_owned(),
    })
}

fn summarize_path(path: &Path) -> Result<SessionSummary, SessionError> {
    let file = File::open(path)?;
    let mut lines = BufReader::new(file).lines();
    let header = parse_header(&mut lines)?;
    let preview = lines
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
        .find_map(|line| {
            let record = line.get("record")?;
            (record.get("type")?.as_str()? == "history_entry")
                .then(|| record.pointer("/entry/message"))
                .flatten()
                .filter(|message| {
                    message.get("role").and_then(serde_json::Value::as_str) == Some("user")
                })
                .and_then(|message| message.get("content"))
                .and_then(serde_json::Value::as_str)
                .map(truncate_preview)
        })
        .unwrap_or_else(|| "Untitled session".to_owned());
    let modified_at = modified_time(path, header.created_at)?;
    Ok(SessionSummary {
        id: header.id,
        created_at: header.created_at,
        modified_at,
        cwd: header.cwd,
        preview,
        model: header.model,
    })
}

fn parse_header(
    lines: &mut impl Iterator<Item = Result<String, std::io::Error>>,
) -> Result<SessionHeader, SessionError> {
    let header_line = lines.next().ok_or_else(|| {
        SessionError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "session header is missing",
        ))
    })??;
    let header: SessionHeader = serde_json::from_str(&header_line)?;
    if header.version != SESSION_VERSION {
        return Err(SessionError::UnsupportedVersion(header.version));
    }
    Ok(header)
}

fn modified_time(path: &Path, fallback: u64) -> Result<u64, SessionError> {
    Ok(fs::metadata(path)?
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(fallback, |duration| duration.as_secs()))
}

fn resolve_path(directory: &Path, requested: &str) -> Result<PathBuf, SessionError> {
    if !directory.exists() {
        return Err(SessionError::NotFound(requested.to_owned()));
    }
    let requested = requested.strip_suffix(".jsonl").unwrap_or(requested);
    let mut matches = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if stem == requested {
            return Ok(path);
        }
        if stem.starts_with(requested) {
            matches.push(path);
        }
    }
    match matches.len() {
        0 => Err(SessionError::NotFound(requested.to_owned())),
        1 => matches.pop().ok_or(SessionError::StatePoisoned),
        _ => Err(SessionError::Ambiguous(requested.to_owned())),
    }
}

fn append_json_line(path: &Path, line: &SessionRecordLine) -> Result<(), SessionError> {
    let mut file = OpenOptions::new().append(true).open(path)?;
    serde_json::to_writer(&mut file, line)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

fn session_id() -> String {
    let suffix = NEXT_SESSION_SUFFIX.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}-{suffix:020}", unix_time(), std::process::id())
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn canonical_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

fn truncate_preview(text: &str) -> String {
    const LIMIT: usize = 80;
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= LIMIT {
        return compact;
    }
    compact.chars().take(LIMIT - 1).collect::<String>() + "…"
}
