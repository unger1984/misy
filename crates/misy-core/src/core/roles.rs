//! Hot-reloaded child-agent role discovery and strict TOML validation.

use crate::{MisyPaths, ModelProfile};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

const MAX_ROLE_TEXT_BYTES: usize = 32 * 1024;

/// Filesystem layer that supplied an effective role.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRoleSource {
    /// Project-local `.misy/agents` definition.
    Project,
    /// Global Misy data-directory definition.
    Global,
    /// Core-provided default.
    BuiltIn,
}

/// Parse and availability status for an effective role name.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRoleStatus {
    /// The role can be selected.
    Available,
    /// A higher-precedence file exists but is malformed or unreadable.
    Invalid,
}

/// Content-free role projection safe to expose to a parent model or client.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRoleSummary {
    /// Stable role name derived from the TOML filename.
    pub name: String,
    /// Bounded parent-facing description.
    pub description: Option<String>,
    /// Winning discovery layer.
    pub source: AgentRoleSource,
    /// Whether the winning definition is usable.
    pub status: AgentRoleStatus,
    /// Bounded warning without file contents.
    pub warning: Option<String>,
}

/// Whether a catalog profile can start without user action.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelAvailability {
    /// Provider, credentials, model, and thinking level are usable.
    Runnable,
    /// Provider exists but needs authentication.
    AuthRequired,
    /// No discovered provider package can serve the model.
    ProviderUnavailable,
    /// The requested profile names an unsupported thinking level.
    UnsupportedThinking,
}

/// Independent cache age/source projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFreshness {
    /// Remote metadata is inside the fifteen-minute TTL.
    Fresh,
    /// Remote metadata is retained past the TTL.
    Stale,
    /// Metadata came from a provider's bundled fallback.
    Bundled,
}

/// One bounded result returned by model-facing catalog search.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelSearchMatch {
    /// Exact catalog-independent selector.
    pub selector: String,
    /// Provider display name.
    pub display_name: String,
    /// Input context tokens, with zero meaning unknown.
    pub context_window: u32,
    /// Bounded provider-supplied description.
    pub description: Option<String>,
    /// Opaque provider-supplied pricing text.
    pub pricing: Option<String>,
    /// Provider default reasoning level.
    pub thinking_default: Option<String>,
    /// Ordered provider-owned reasoning levels.
    pub thinking_levels: Vec<String>,
    /// Whether this model/profile appears in the selected role.
    pub in_role: bool,
    /// Runtime availability independent from cache age.
    pub availability: ModelAvailability,
    /// Cache age/source independent from runtime availability.
    pub freshness: ModelFreshness,
    /// Bounded non-secret diagnostics.
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct AgentRoleSnapshot {
    pub(crate) summary: AgentRoleSummary,
    pub(crate) instructions: Option<String>,
    pub(crate) models: Option<Vec<ModelProfile>>,
    pub(crate) tools: Option<BTreeSet<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleToml {
    version: u32,
    description: Option<String>,
    instructions: Option<String>,
    models: Option<Vec<String>>,
    tools: Option<Vec<String>>,
}

pub(crate) fn discover(
    paths: &MisyPaths,
    project_root: &Path,
    known_tools: &BTreeSet<String>,
) -> BTreeMap<String, AgentRoleSnapshot> {
    let mut roles = BTreeMap::new();
    load_directory(
        &project_root.join(".misy").join("agents"),
        AgentRoleSource::Project,
        known_tools,
        &mut roles,
    );
    load_directory(
        &paths.agents_dir(),
        AgentRoleSource::Global,
        known_tools,
        &mut roles,
    );
    for role in builtins() {
        roles.entry(role.summary.name.clone()).or_insert(role);
    }
    roles
}

pub(crate) fn parent_catalog_description(roles: &BTreeMap<String, AgentRoleSnapshot>) -> String {
    roles
        .values()
        .map(|role| {
            let status = match role.summary.status {
                AgentRoleStatus::Available => "available",
                AgentRoleStatus::Invalid => "unavailable",
            };
            let description = role
                .summary
                .description
                .as_deref()
                .unwrap_or("No description");
            format!("{} ({status}): {description}", role.summary.name)
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn load_directory(
    directory: &Path,
    source: AgentRoleSource,
    known_tools: &BTreeSet<String>,
    roles: &mut BTreeMap<String, AgentRoleSnapshot>,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("toml"))
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let Some(name) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if roles.contains_key(name) || !valid_role_name(name) {
            continue;
        }
        let snapshot = parse_role(&path, name, source, known_tools).unwrap_or_else(|warning| {
            AgentRoleSnapshot {
                summary: AgentRoleSummary {
                    name: name.to_owned(),
                    description: None,
                    source,
                    status: AgentRoleStatus::Invalid,
                    warning: Some(warning),
                },
                instructions: None,
                models: None,
                tools: None,
            }
        });
        roles.insert(name.to_owned(), snapshot);
    }
}

fn parse_role(
    path: &PathBuf,
    name: &str,
    source: AgentRoleSource,
    known_tools: &BTreeSet<String>,
) -> Result<AgentRoleSnapshot, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("could not read role `{name}`: {error}"))?;
    let role: RoleToml = toml::from_str(&contents)
        .map_err(|error| format!("could not parse role `{name}`: {error}"))?;
    if role.version != 1 {
        return Err(format!(
            "role `{name}` requires unsupported version {}",
            role.version
        ));
    }
    let description = bounded(role.description, "description")?;
    let instructions = bounded(role.instructions, "instructions")?;
    let models = role.models.map(parse_models).transpose()?;
    let tools = role
        .tools
        .map(|tools| validate_tools(tools, known_tools))
        .transpose()?;
    Ok(AgentRoleSnapshot {
        summary: AgentRoleSummary {
            name: name.to_owned(),
            description,
            source,
            status: AgentRoleStatus::Available,
            warning: None,
        },
        instructions,
        models,
        tools,
    })
}

fn bounded(value: Option<String>, field: &str) -> Result<Option<String>, String> {
    value
        .map(|value| {
            if value.len() > MAX_ROLE_TEXT_BYTES {
                Err(format!("role {field} exceeds {MAX_ROLE_TEXT_BYTES} bytes"))
            } else if value.trim().is_empty() {
                Err(format!("role {field} must not be empty"))
            } else {
                Ok(value)
            }
        })
        .transpose()
}

fn parse_models(values: Vec<String>) -> Result<Vec<ModelProfile>, String> {
    if values.is_empty() {
        return Err("role models must not be empty".to_owned());
    }
    let profiles = values
        .into_iter()
        .map(|value| ModelProfile::from_str(&value).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    if profiles.iter().collect::<BTreeSet<_>>().len() != profiles.len() {
        return Err("role models must not contain exact duplicates".to_owned());
    }
    Ok(profiles)
}

fn validate_tools(
    tools: Vec<String>,
    known_tools: &BTreeSet<String>,
) -> Result<BTreeSet<String>, String> {
    let tools = tools.into_iter().collect::<BTreeSet<_>>();
    if let Some(unknown) = tools.iter().find(|tool| !known_tools.contains(*tool)) {
        return Err(format!("role references unknown tool `{unknown}`"));
    }
    Ok(tools)
}

fn valid_role_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

fn builtins() -> Vec<AgentRoleSnapshot> {
    [
        ("default", "General delegated work", None, None),
        (
            "explorer",
            "Bounded read-only codebase research",
            Some("Research the requested area and report a concise evidence map."),
            Some(["list_directory", "read_file", "view_image"].as_slice()),
        ),
        (
            "worker",
            "Bounded implementation with focused verification",
            Some("Own the requested package, preserve unrelated changes, and verify the result."),
            None,
        ),
    ]
    .into_iter()
    .map(
        |(name, description, instructions, tools)| AgentRoleSnapshot {
            summary: AgentRoleSummary {
                name: name.to_owned(),
                description: Some(description.to_owned()),
                source: AgentRoleSource::BuiltIn,
                status: AgentRoleStatus::Available,
                warning: None,
            },
            instructions: instructions.map(ToOwned::to_owned),
            models: None,
            tools: tools.map(|tools| tools.iter().map(|tool| (*tool).to_owned()).collect()),
        },
    )
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_invalid_role_tombstones_global_and_hot_reload_recovers() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let paths = MisyPaths::from_root(temporary.path().join("data"));
        let project = temporary.path().join("project");
        let global_agents = paths.agents_dir();
        let project_agents = project.join(".misy/agents");
        fs::create_dir_all(&global_agents).expect("global agents directory");
        fs::create_dir_all(&project_agents).expect("project agents directory");
        fs::write(
            global_agents.join("reviewer.toml"),
            "version = 1\ndescription = \"global\"\n",
        )
        .expect("global role");
        fs::write(
            project_agents.join("reviewer.toml"),
            "version = 1\ntools = [\"unknown\"]\n",
        )
        .expect("invalid project role");
        let tools = BTreeSet::from(["read_file".to_owned()]);

        let invalid = discover(&paths, &project, &tools);
        let reviewer = invalid.get("reviewer").expect("reviewer role");
        assert_eq!(reviewer.summary.source, AgentRoleSource::Project);
        assert_eq!(reviewer.summary.status, AgentRoleStatus::Invalid);

        fs::write(
            project_agents.join("reviewer.toml"),
            "version = 1\ndescription = \"project\"\ntools = [\"read_file\"]\n",
        )
        .expect("fixed project role");
        let recovered = discover(&paths, &project, &tools);
        let reviewer = recovered.get("reviewer").expect("reviewer role");
        assert_eq!(reviewer.summary.status, AgentRoleStatus::Available);
        assert_eq!(reviewer.summary.description.as_deref(), Some("project"));
        assert_eq!(
            reviewer.tools,
            Some(BTreeSet::from(["read_file".to_owned()]))
        );
    }

    #[test]
    fn duplicate_exact_profiles_are_invalid_but_distinct_levels_are_allowed() {
        let duplicate = parse_models(vec!["openai/model:low".into(), "openai/model:low".into()]);
        assert!(duplicate.is_err());
        let distinct = parse_models(vec!["openai/model:low".into(), "openai/model:high".into()]);
        assert_eq!(distinct.expect("distinct profiles").len(), 2);
    }
}
