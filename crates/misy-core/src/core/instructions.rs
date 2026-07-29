//! Hierarchical `AGENTS.md` discovery, stable session caches, and request rendering.

use super::instruction_contracts::{
    ContextReportState, InstructionOwner, InstructionScope, InstructionSourceKind,
    InstructionSourceStatus, InstructionSourceSummary, InstructionWarning,
    InstructionWarningReason,
};
use crate::MisyPaths;
use crate::core::instruction_paths::ResolvedTarget;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

mod render;

use render::{append_unbudgeted, render_project_chain};

const SOURCE_LIMIT: usize = 32 * 1024;
const PROJECT_CHAIN_LIMIT: usize = 32 * 1024;
const MAX_SOURCE_SLOTS: usize = 256;
const MAX_CACHED_BYTES: usize = 8 * 1024 * 1024;
const MAX_METADATA_FIELD_BYTES: usize = 1024;

pub(crate) const BUILTIN_INSTRUCTIONS: &str = concat!(
    "You are Misy, a coding agent. Direct user instructions override AGENTS.md. ",
    "Project AGENTS.md files apply from their directory downward, and deeper files override ",
    "parents. Resolve paths mentioned by an AGENTS.md relative to that file. Read only routed ",
    "documents needed for the current work. Before filesystem work, use a tool cwd/path inside ",
    "the intended scope. Shell commands are scoped only by exec_command.cwd; Misy does not parse ",
    "paths embedded in shell text."
);

pub(crate) const CHILD_INSTRUCTIONS: &str = concat!(
    "You are a child agent working on a delegated task. Work independently, return a concise ",
    "result to your parent, and never speak to the root user as that user's assistant."
);

#[derive(Debug)]
struct CachedSource {
    summary: InstructionSourceSummary,
    content: Option<Arc<str>>,
    warning: Option<InstructionWarningReason>,
}

#[derive(Debug)]
enum NestedEntry {
    Missing,
    Source(Arc<CachedSource>),
}

#[derive(Debug, Default)]
struct ConversationBudget {
    slots: usize,
    bytes: usize,
}

#[derive(Debug)]
pub(crate) struct InstructionRoot {
    workspace_cwd: PathBuf,
    project_root: PathBuf,
    base: Vec<Arc<CachedSource>>,
    budget: Arc<Mutex<ConversationBudget>>,
}

#[derive(Debug)]
pub(crate) struct InstructionSession {
    root: Arc<InstructionRoot>,
    owner: InstructionOwner,
    nested: BTreeMap<PathBuf, NestedEntry>,
    active: BTreeSet<PathBuf>,
    submission_active: bool,
    last: Vec<InstructionSourceSummary>,
    last_warnings: Vec<InstructionWarning>,
    reported_warning_keys: BTreeSet<String>,
    reserved_slots: usize,
    reserved_bytes: usize,
}

#[derive(Debug)]
pub(crate) struct InstructionRuntime {
    root: Arc<InstructionRoot>,
    main: Arc<Mutex<InstructionSession>>,
}

/// Result of discovering scopes for a validated tool batch.
pub(crate) struct ScopeDiscovery {
    pub(crate) activated: bool,
    pub(crate) error: Option<String>,
    pub(crate) warnings: Vec<InstructionWarning>,
}

impl InstructionRoot {
    pub(crate) fn load(paths: &MisyPaths, strict: bool) -> Result<Arc<Self>, String> {
        let workspace_cwd = std::env::current_dir()
            .and_then(|path| path.canonicalize())
            .map_err(|error| format!("could not resolve workspace cwd: {error}"))?;
        Self::load_at(paths, strict, &workspace_cwd)
    }

    pub(crate) fn load_at(
        paths: &MisyPaths,
        strict: bool,
        workspace_cwd: &Path,
    ) -> Result<Arc<Self>, String> {
        let workspace_cwd = workspace_cwd
            .canonicalize()
            .map_err(|error| format!("could not resolve workspace cwd: {error}"))?;
        let project_root = find_project_root(&workspace_cwd);
        let budget = Arc::new(Mutex::new(ConversationBudget::default()));
        let candidates = [
            (
                paths.root().join("AGENTS.md"),
                InstructionSourceKind::Global,
                InstructionScope::Global,
            ),
            (
                project_root.join("AGENTS.md"),
                InstructionSourceKind::ProjectRoot,
                InstructionScope::ProjectRoot(display_path(&project_root)),
            ),
        ];
        let mut base = Vec::new();
        for (path, kind, scope) in candidates {
            let Some(source) = load_source(&path, kind, scope, InstructionOwner::Main) else {
                continue;
            };
            if strict && source.warning.is_some() {
                return Err(source_error(&source));
            }
            reserve_base(&budget, &source)?;
            base.push(Arc::new(source));
        }
        Ok(Arc::new(Self {
            workspace_cwd,
            project_root,
            base,
            budget,
        }))
    }

    pub(crate) fn workspace_cwd(&self) -> &Path {
        &self.workspace_cwd
    }

    pub(crate) fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub(crate) fn base_error(&self) -> Option<String> {
        self.base
            .iter()
            .find(|source| source.warning.is_some())
            .map(|source| source_error(source))
    }
}

impl InstructionRuntime {
    pub(crate) fn new(root: Arc<InstructionRoot>) -> Self {
        let main = Arc::new(Mutex::new(InstructionSession::new(
            Arc::clone(&root),
            InstructionOwner::Main,
        )));
        Self { root, main }
    }

    pub(crate) fn main(&self) -> Arc<Mutex<InstructionSession>> {
        Arc::clone(&self.main)
    }

    pub(crate) fn root(&self) -> Arc<InstructionRoot> {
        Arc::clone(&self.root)
    }

    pub(crate) fn replace(&mut self, root: Arc<InstructionRoot>) {
        *self = Self::new(root);
    }
}

impl InstructionSession {
    pub(crate) fn new(root: Arc<InstructionRoot>, owner: InstructionOwner) -> Self {
        Self {
            root,
            owner,
            nested: BTreeMap::new(),
            active: BTreeSet::new(),
            submission_active: false,
            last: Vec::new(),
            last_warnings: Vec::new(),
            reported_warning_keys: BTreeSet::new(),
            reserved_slots: 0,
            reserved_bytes: 0,
        }
    }

    pub(crate) fn root(&self) -> Arc<InstructionRoot> {
        Arc::clone(&self.root)
    }

    pub(crate) fn begin_submission(&mut self) {
        self.active.clear();
        self.reported_warning_keys.clear();
        self.submission_active = true;
    }

    pub(crate) fn finish_submission(&mut self) {
        let rendered = self.render(false);
        self.last = rendered.summaries;
        self.last_warnings = rendered.warnings;
        self.active.clear();
        self.submission_active = false;
    }

    pub(crate) fn rendered(&self, child: bool) -> RenderedInstructions {
        self.render(child)
    }

    pub(crate) fn report_sources(
        &self,
    ) -> (
        ContextReportState,
        Vec<InstructionSourceSummary>,
        Vec<InstructionWarning>,
    ) {
        if self.submission_active {
            let rendered = self.render(false);
            return (
                ContextReportState::ActiveSubmission,
                rendered.summaries,
                rendered.warnings,
            );
        }
        if !self.last.is_empty() {
            return (
                ContextReportState::LastSubmission,
                self.last.clone(),
                self.last_warnings.clone(),
            );
        }
        let rendered = self.render(false);
        (
            ContextReportState::Base,
            rendered.summaries,
            rendered.warnings,
        )
    }

    pub(crate) fn discover(&mut self, targets: &[ResolvedTarget]) -> ScopeDiscovery {
        let mut activated = false;
        let mut warnings = Vec::new();
        let mut error = None;
        let mut candidates = BTreeSet::new();
        for target in targets {
            if !target.scope_directory.starts_with(self.root.project_root()) {
                continue;
            }
            candidates.extend(nested_directories(
                self.root.project_root(),
                &target.scope_directory,
            ));
        }
        for directory in candidates {
            let path = directory.join("AGENTS.md");
            if !self.nested.contains_key(&path) {
                let entry = match load_source(
                    &path,
                    InstructionSourceKind::Nested,
                    InstructionScope::Nested(display_path(&directory)),
                    self.owner,
                ) {
                    None => NestedEntry::Missing,
                    Some(mut source) => {
                        if let Err(reason) = self.reserve_nested(&source) {
                            source.content = None;
                            source.summary.status = InstructionSourceStatus::Blocked;
                            source.summary.retained_bytes = 0;
                            source.warning = Some(reason);
                        }
                        NestedEntry::Source(Arc::new(source))
                    }
                };
                self.nested.insert(path.clone(), entry);
            }
            let Some(NestedEntry::Source(source)) = self.nested.get(&path) else {
                continue;
            };
            let newly_active = self.active.insert(path);
            if newly_active {
                activated = source.content.is_some() || source.warning.is_some() || activated;
            }
            if let Some(reason) = &source.warning {
                let warning = warning(source, reason.clone(), self.owner);
                error.get_or_insert_with(|| source_error(source));
                if newly_active && !warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
        }
        if activated && error.is_none() {
            for warning in self.render(false).warnings {
                if !warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
        }
        warnings.retain(|warning| self.reported_warning_keys.insert(warning_key(warning)));
        ScopeDiscovery {
            activated,
            error,
            warnings,
        }
    }

    fn reserve_nested(&mut self, source: &CachedSource) -> Result<(), InstructionWarningReason> {
        let bytes = source.content.as_ref().map_or(0, |content| content.len());
        let mut budget = self
            .root
            .budget
            .lock()
            .expect("instruction budget mutex must not be poisoned");
        if budget.slots.saturating_add(1) > MAX_SOURCE_SLOTS
            || budget.bytes.saturating_add(bytes) > MAX_CACHED_BYTES
        {
            return Err(InstructionWarningReason::ConversationLimit);
        }
        budget.slots += 1;
        budget.bytes += bytes;
        self.reserved_slots += 1;
        self.reserved_bytes += bytes;
        Ok(())
    }

    fn render(&self, child: bool) -> RenderedInstructions {
        let mut sections = vec![BUILTIN_INSTRUCTIONS.to_owned()];
        if child {
            sections.push(CHILD_INSTRUCTIONS.to_owned());
        }
        let mut summaries = Vec::new();
        let mut warnings = Vec::new();
        if let Some(global) = self
            .root
            .base
            .iter()
            .find(|source| source.summary.kind == InstructionSourceKind::Global)
        {
            append_unbudgeted(
                global,
                self.owner,
                &mut sections,
                &mut summaries,
                &mut warnings,
            );
        }
        let mut project = self
            .root
            .base
            .iter()
            .filter(|source| source.summary.kind == InstructionSourceKind::ProjectRoot)
            .cloned()
            .collect::<Vec<_>>();
        project.extend(
            self.active
                .iter()
                .filter_map(|path| match self.nested.get(path) {
                    Some(NestedEntry::Source(source)) => Some(Arc::clone(source)),
                    _ => None,
                }),
        );
        let rendered = render_project_chain(&project, self.owner);
        sections.extend(rendered.sections);
        summaries.extend(rendered.summaries);
        warnings.extend(rendered.warnings);
        RenderedInstructions {
            system_prompt: sections.join("\n\n"),
            summaries,
            warnings,
        }
    }
}

impl Drop for InstructionSession {
    fn drop(&mut self) {
        let Ok(mut budget) = self.root.budget.lock() else {
            return;
        };
        budget.slots = budget.slots.saturating_sub(self.reserved_slots);
        budget.bytes = budget.bytes.saturating_sub(self.reserved_bytes);
    }
}

pub(crate) struct RenderedInstructions {
    pub(crate) system_prompt: String,
    pub(crate) summaries: Vec<InstructionSourceSummary>,
    pub(crate) warnings: Vec<InstructionWarning>,
}

fn find_project_root(workspace: &Path) -> PathBuf {
    workspace
        .ancestors()
        .find(|directory| directory.join(".git").exists())
        .map_or_else(|| workspace.to_path_buf(), Path::to_path_buf)
}

fn nested_directories(root: &Path, target: &Path) -> Vec<PathBuf> {
    let Ok(relative) = target.strip_prefix(root) else {
        return Vec::new();
    };
    let mut directories = Vec::new();
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        directories.push(current.clone());
    }
    directories
}

fn load_source(
    path: &Path,
    kind: InstructionSourceKind,
    scope: InstructionScope,
    owner: InstructionOwner,
) -> Option<CachedSource> {
    let link_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => return Some(blocked(path, kind, scope, owner, io_reason(&error))),
    };
    let metadata = if link_metadata.file_type().is_symlink() {
        match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => return Some(blocked(path, kind, scope, owner, io_reason(&error))),
        }
    } else {
        link_metadata
    };
    if !metadata.is_file() {
        return Some(blocked_with_bytes(
            path,
            kind,
            scope,
            owner,
            InstructionWarningReason::NotRegularFile,
            usize::try_from(metadata.len()).unwrap_or(usize::MAX),
        ));
    }
    if metadata.len() > SOURCE_LIMIT as u64 {
        return Some(blocked_with_bytes(
            path,
            kind,
            scope,
            owner,
            InstructionWarningReason::SourceTooLarge,
            usize::try_from(metadata.len()).unwrap_or(usize::MAX),
        ));
    }
    let bytes = match read_bounded(path) {
        Ok(bytes) => bytes,
        Err(error) => return Some(blocked(path, kind, scope, owner, io_reason(&error))),
    };
    if bytes.len() > SOURCE_LIMIT {
        return Some(blocked_with_bytes(
            path,
            kind,
            scope,
            owner,
            InstructionWarningReason::SourceTooLarge,
            bytes.len(),
        ));
    }
    let content = match String::from_utf8(bytes) {
        Ok(content) => content,
        Err(error) => {
            return Some(blocked_with_bytes(
                path,
                kind,
                scope,
                owner,
                InstructionWarningReason::InvalidUtf8,
                error.as_bytes().len(),
            ));
        }
    };
    if content.trim().is_empty() {
        return None;
    }
    let length = content.len();
    Some(CachedSource {
        summary: InstructionSourceSummary {
            kind,
            display_path: display_path(path),
            scope,
            original_bytes: length,
            retained_bytes: length,
            truncated: false,
            status: InstructionSourceStatus::Active,
        },
        content: Some(Arc::from(content)),
        warning: None,
    })
}

fn blocked(
    path: &Path,
    kind: InstructionSourceKind,
    scope: InstructionScope,
    _owner: InstructionOwner,
    reason: InstructionWarningReason,
) -> CachedSource {
    blocked_with_bytes(path, kind, scope, _owner, reason, 0)
}

fn blocked_with_bytes(
    path: &Path,
    kind: InstructionSourceKind,
    scope: InstructionScope,
    _owner: InstructionOwner,
    reason: InstructionWarningReason,
    original_bytes: usize,
) -> CachedSource {
    CachedSource {
        summary: InstructionSourceSummary {
            kind,
            display_path: display_path(path),
            scope,
            original_bytes,
            retained_bytes: 0,
            truncated: false,
            status: InstructionSourceStatus::Blocked,
        },
        content: None,
        warning: Some(reason),
    }
}

fn read_bounded(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take((SOURCE_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn reserve_base(budget: &Mutex<ConversationBudget>, source: &CachedSource) -> Result<(), String> {
    let bytes = source.content.as_ref().map_or(0, |content| content.len());
    let mut budget = budget
        .lock()
        .expect("instruction budget mutex must not be poisoned");
    if budget.slots.saturating_add(1) > MAX_SOURCE_SLOTS
        || budget.bytes.saturating_add(bytes) > MAX_CACHED_BYTES
    {
        return Err("base AGENTS.md sources exceed the root-conversation budget".to_owned());
    }
    budget.slots += 1;
    budget.bytes += bytes;
    Ok(())
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .flat_map(char::escape_default)
        .collect()
}

fn utf8_prefix(text: &str, maximum: usize) -> &str {
    let mut end = maximum.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn warning(
    source: &CachedSource,
    reason: InstructionWarningReason,
    owner: InstructionOwner,
) -> InstructionWarning {
    InstructionWarning {
        source: source.summary.clone(),
        reason,
        owner,
    }
}

fn warning_key(warning: &InstructionWarning) -> String {
    format!(
        "{:?}|{}|{:?}",
        warning.owner, warning.source.display_path, warning.reason
    )
}

fn source_error(source: &CachedSource) -> String {
    format!(
        "instruction source `{}` is blocked: {:?}",
        source.summary.display_path, source.warning
    )
}

fn io_reason(error: &std::io::Error) -> InstructionWarningReason {
    InstructionWarningReason::Io(error.to_string())
}

#[cfg(test)]
#[path = "instructions_tests.rs"]
mod tests;
