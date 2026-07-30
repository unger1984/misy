//! Unified routing for core-owned tools that need session or agent state.

use super::{CoreState, agents, questions, todos, turn};
use crate::{ToolCall, ToolResult};
use std::{future::Future, pin::Pin};

pub(crate) fn is_core_tool(name: &str) -> bool {
    matches!(name, "SetTodoList" | "AskUserQuestion") || agents::is_agent_tool(name)
}

pub(crate) fn dispatch<'a>(
    core: &'a CoreState,
    state: &'a turn::AgentTurnState,
    call: &'a ToolCall,
) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
    Box::pin(async move {
        if state
            .allowed_tools()
            .is_some_and(|tools| !tools.contains(&call.name))
        {
            return ToolResult::error(
                &call.id,
                format!("tool `{}` is not allowed for this agent role", call.name),
            );
        }
        if let Err(error) = core.dispatcher.validate_arguments(call) {
            return ToolResult::error(&call.id, error.to_string());
        }
        if agents::is_agent_tool(&call.name) {
            return agents::dispatch_agent_tool(core, state, call).await;
        }
        match call.name.as_str() {
            "SetTodoList" => todos::dispatch(core, state, call),
            "AskUserQuestion" => questions::dispatch(core, state, call).await,
            _ => ToolResult::error(&call.id, format!("tool `{}` is not core-owned", call.name)),
        }
    })
}
