//! Role resolution, history forking, and model-profile preflight for child spawning.

use super::spawn::SpawnModel;
use crate::{
    AgentRoleStatus, CoreError, HistoryEntry, ModelProfile,
    core::{CoreState, turn},
};
use std::collections::BTreeSet;

pub(super) struct ResolvedRole {
    pub(super) name: String,
    pub(super) models: Option<Vec<ModelProfile>>,
    pub(super) instructions: Option<String>,
    pub(super) tools: Option<BTreeSet<String>>,
}

#[derive(Clone, Copy)]
pub(super) enum ForkTurns {
    None,
    All,
    Last(usize),
}

pub(super) fn parse_fork_turns(value: &str) -> Result<ForkTurns, String> {
    match value {
        "none" => Ok(ForkTurns::None),
        "all" => Ok(ForkTurns::All),
        value => value
            .parse::<usize>()
            .ok()
            .filter(|turns| *turns > 0)
            .map(ForkTurns::Last)
            .ok_or_else(|| "fork_turns must be `none`, `all`, or a positive integer".to_owned()),
    }
}

pub(super) fn resolve_role(
    core: &CoreState,
    parent: &turn::AgentTurnState,
    requested: Option<&str>,
    fork: ForkTurns,
) -> Result<ResolvedRole, String> {
    if matches!(fork, ForkTurns::All) {
        return Ok(ResolvedRole {
            name: parent.role().unwrap_or("default").to_owned(),
            models: None,
            instructions: parent.role_instructions().map(ToOwned::to_owned),
            tools: parent.allowed_tools().cloned(),
        });
    }
    let roles = core.discover_roles();
    let name = requested.unwrap_or("default").to_owned();
    let role = roles
        .get(&name)
        .ok_or_else(|| format!("unknown agent_type `{name}`"))?;
    if role.summary.status == AgentRoleStatus::Invalid {
        return Err(role
            .summary
            .warning
            .clone()
            .unwrap_or_else(|| format!("agent role `{name}` is invalid")));
    }
    Ok(ResolvedRole {
        name,
        models: role.models.clone(),
        instructions: role.instructions.clone(),
        tools: role.tools.clone(),
    })
}

pub(super) fn fork_prefix(prefix: &[HistoryEntry], fork: ForkTurns) -> Vec<HistoryEntry> {
    match fork {
        ForkTurns::None => Vec::new(),
        ForkTurns::All => prefix.to_vec(),
        ForkTurns::Last(turns) => {
            let start = prefix
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, entry)| entry.message.role == crate::MessageRole::User)
                .nth(turns.saturating_sub(1))
                .map_or(0, |(index, _)| index);
            prefix[start..].to_vec()
        }
    }
}

pub(super) fn profile_candidates(
    parent: &turn::AgentTurnState,
    role: Option<&[ModelProfile]>,
    requested: Option<SpawnModel>,
) -> Result<Vec<ModelProfile>, String> {
    match requested {
        Some(SpawnModel::Strict(selector)) => selector
            .parse()
            .map(|profile| vec![profile])
            .map_err(|error: crate::ModelSelectorError| error.to_string()),
        Some(SpawnModel::Chain(selectors)) => {
            if selectors.is_empty() {
                return Err("model fallback chain must not be empty".to_owned());
            }
            selectors
                .into_iter()
                .map(|selector| {
                    selector
                        .parse()
                        .map_err(|error: crate::ModelSelectorError| error.to_string())
                })
                .collect()
        }
        None => inherited_or_role_profiles(parent, role),
    }
}

fn inherited_or_role_profiles(
    parent: &turn::AgentTurnState,
    role: Option<&[ModelProfile]>,
) -> Result<Vec<ModelProfile>, String> {
    let Some(role) = role else {
        return Ok(vec![ModelProfile::new(
            parent.model().clone(),
            parent.thinking().map(ToOwned::to_owned),
        )]);
    };
    if role.len() == 1 {
        return Ok(role.to_vec());
    }
    let matching = role
        .iter()
        .filter(|profile| profile.model.provider == parent.model().provider)
        .count();
    if matching != 1 {
        return Err(
            "model_selection_required: call model_search and pass an exact model selector"
                .to_owned(),
        );
    }
    let primary = role
        .iter()
        .find(|profile| profile.model.provider == parent.model().provider)
        .expect("one matching role profile was counted");
    let mut ordered = vec![primary.clone()];
    ordered.extend(role.iter().filter(|profile| *profile != primary).cloned());
    Ok(ordered)
}

pub(super) async fn viable_profiles(
    core: &CoreState,
    candidates: &[ModelProfile],
) -> Result<Vec<ModelProfile>, String> {
    let mut failures = Vec::new();
    let mut viable = Vec::new();
    for profile in candidates {
        match validate_profile(core, profile).await {
            Ok(()) => viable.push(profile.clone()),
            Err(error) => failures.push(format!("{}: {error}", profile.selector())),
        }
    }
    if viable.is_empty() {
        Err(format!("model fallback exhausted: {}", failures.join("; ")))
    } else {
        Ok(viable)
    }
}

async fn validate_profile(core: &CoreState, profile: &ModelProfile) -> Result<(), CoreError> {
    if !core.credential_epoch(&profile.model.provider).present {
        return Err(CoreError::AgentAuthenticationRequired(
            profile.model.provider.clone(),
        ));
    }
    let mut cached = core.model_cache.load();
    if !cached
        .iter()
        .any(|candidate| candidate.model == profile.model)
    {
        cached = core.fetch_models(&profile.model.provider).await?.models;
    }
    let Some(info) = cached
        .iter()
        .find(|candidate| candidate.model == profile.model)
    else {
        return Err(CoreError::AgentModelUnavailable(profile.model.clone()));
    };
    if let Some(thinking) = profile.thinking.as_deref() {
        let capability = core
            .catalog
            .get(&profile.model.provider)
            .is_some_and(|package| package.manifest().supports_capability("thinking", 1));
        let supported = info
            .thinking
            .as_ref()
            .is_some_and(|metadata| metadata.levels.iter().any(|level| level.id == thinking));
        if !supported || !capability {
            return Err(CoreError::UnsupportedThinking(profile.clone()));
        }
    }
    Ok(())
}

pub(super) fn intersect_tools(
    parent: Option<&BTreeSet<String>>,
    role: Option<&BTreeSet<String>>,
) -> Option<BTreeSet<String>> {
    match (parent, role) {
        (None, None) => None,
        (Some(parent), None) => Some(parent.clone()),
        (None, Some(role)) => Some(role.clone()),
        (Some(parent), Some(role)) => Some(parent.intersection(role).cloned().collect()),
    }
}
