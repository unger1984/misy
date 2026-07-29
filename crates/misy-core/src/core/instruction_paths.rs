//! Canonical workspace-relative target resolution for local filesystem tools.

use std::path::{Component, Path, PathBuf};

/// Canonical target and directory used for instruction-scope discovery.
#[derive(Clone, Debug)]
pub(crate) struct ResolvedTarget {
    pub(crate) path: PathBuf,
    pub(crate) scope_directory: PathBuf,
}

pub(crate) fn resolve_target(workspace_cwd: &Path, raw: &str) -> Result<ResolvedTarget, String> {
    let joined = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        workspace_cwd.join(raw)
    };
    let normalized = normalize_lexically(&joined)?;
    let path = canonicalize_allow_missing(&normalized)?;
    let scope_directory = if path.is_dir() {
        path.clone()
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.clone())
    };
    Ok(ResolvedTarget {
        path,
        scope_directory,
    })
}

fn normalize_lexically(path: &Path) -> Result<PathBuf, String> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(format!(
                        "path `{}` escapes the filesystem root",
                        path.display()
                    ));
                }
            }
        }
    }
    Ok(normalized)
}

fn canonicalize_allow_missing(path: &Path) -> Result<PathBuf, String> {
    let mut ancestor = path;
    let mut tail = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            return Err(format!(
                "path `{}` has no existing ancestor",
                path.display()
            ));
        };
        tail.push(name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            return Err(format!(
                "path `{}` has no existing ancestor",
                path.display()
            ));
        };
        ancestor = parent;
    }
    let mut resolved = ancestor
        .canonicalize()
        .map_err(|error| format!("could not resolve `{}`: {error}", path.display()))?;
    for component in tail.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::resolve_target;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn resolves_missing_targets_from_the_nearest_existing_ancestor() {
        let temp = TempDir::new().expect("tempdir");
        fs::create_dir(temp.path().join("existing")).expect("existing directory");
        let root = temp.path().canonicalize().expect("canonical root");
        let target = resolve_target(&root, "existing/new/file.rs").expect("target");

        assert_eq!(target.path, root.join("existing/new/file.rs"));
        assert_eq!(target.scope_directory, root.join("existing/new"));
    }
}
