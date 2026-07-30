//! Canonical agent addressing and descendant-tree cancellation.

use super::{AgentId, AgentRegistry};
use crate::CoreError;

impl AgentRegistry {
    pub(super) fn resolve_target(
        &self,
        caller: Option<AgentId>,
        target: &str,
    ) -> Result<AgentId, String> {
        if let Some(number) = target
            .strip_prefix("agent-")
            .and_then(|suffix| suffix.parse::<u64>().ok())
            .filter(|number| *number > 0)
        {
            let id = AgentId::new(number);
            return self
                .record(id)
                .map(|_| id)
                .map_err(|error| error.to_string());
        }
        self.list()
            .into_iter()
            .find(|summary| {
                if target.starts_with('/') {
                    summary.path == target
                } else {
                    summary.parent == caller && summary.task_name == target
                }
            })
            .map(|summary| summary.id)
            .ok_or_else(|| format!("unknown agent target `{target}`"))
    }

    pub(crate) fn stop_tree(&self, id: AgentId) -> Result<(), CoreError> {
        let root_path = self.record(id)?.summary().path;
        self.stop_matching_paths(&root_path, true);
        Ok(())
    }

    pub(crate) fn stop_descendants(&self, id: AgentId) -> Result<(), CoreError> {
        let root_path = self.record(id)?.summary().path;
        self.stop_matching_paths(&root_path, false);
        Ok(())
    }

    fn stop_matching_paths(&self, root_path: &str, include_root: bool) {
        let descendant_prefix = format!("{root_path}/");
        let mut descendants = self
            .list()
            .into_iter()
            .filter(|summary| {
                (include_root && summary.path == root_path)
                    || summary.path.starts_with(&descendant_prefix)
            })
            .collect::<Vec<_>>();
        descendants.sort_by_key(|summary| std::cmp::Reverse(summary.path.len()));
        for summary in descendants {
            if !summary.status.is_terminal()
                && let Ok(record) = self.record(summary.id)
            {
                record.cancel();
            }
        }
    }
}
