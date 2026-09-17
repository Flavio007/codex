//! Routing decisions and subtask decomposition parser for the Architect phase.

use serde::Deserialize;
use serde::Serialize;

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;

/// Complexity tier assigned by the Architect for each delegated worker subtask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerComplexity {
    /// Low complexity: simple edits, refactoring, docstrings, formatting (e.g. Luna).
    Trivial,
    /// Standard complexity: component implementations, feature logic, integration (e.g. Terra).
    Normal,
    /// High complexity: deep algorithmic work, concurrent/unsafe logic, low-level optimization (e.g. Sol).
    Difficult,
}

impl WorkerComplexity {
    /// Maps complexity tier to the standard designated model name.
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Trivial => "gpt-5.6-luna",
            Self::Normal => "gpt-5.6-terra",
            Self::Difficult => "gpt-5.6-sol",
        }
    }

    /// Resolves target model, falling back to configured worker model when appropriate.
    pub fn resolve_model(self, configured_worker_model: &str) -> String {
        match self {
            Self::Trivial => "gpt-5.6-luna".to_string(),
            Self::Normal => {
                if configured_worker_model.is_empty() {
                    "gpt-5.6-terra".to_string()
                } else {
                    configured_worker_model.to_string()
                }
            }
            Self::Difficult => "gpt-5.6-sol".to_string(),
        }
    }

    /// Parses complexity from string tokens.
    pub fn parse_str(s: &str) -> Self {
        let lower = s.trim().to_lowercase();
        if lower.contains("trivial") || lower.contains("facil") || lower.contains("simple") {
            Self::Trivial
        } else if lower.contains("difficult")
            || lower.contains("dificil")
            || lower.contains("hard")
            || lower.contains("complex")
        {
            Self::Difficult
        } else {
            Self::Normal
        }
    }
}

/// A structured subtask delegated to an isolated worker agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subtask {
    pub name: String,
    pub complexity: WorkerComplexity,
    pub relevant_context: String,
    pub task: String,
    pub constraints: String,
    pub acceptance_tests: String,
}

/// Structured decision returned by the Architect (Astra).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutingDecision {
    /// Astra implements the solution directly without delegating to workers.
    Direct {
        raw_response: String,
    },
    /// Astra delegates the solution to one or more workers based on a task plan.
    Delegate {
        plan_summary: String,
        tasks: Vec<Subtask>,
    },
}

impl RoutingDecision {
    pub fn is_direct(&self) -> bool {
        matches!(self, Self::Direct { .. })
    }

    pub fn is_delegate(&self) -> bool {
        matches!(self, Self::Delegate { .. })
    }
}

/// Parses Astra's response into a structured `RoutingDecision`.
pub fn parse_routing_decision(response: &str) -> RoutingDecision {
    let trimmed = response.trim();

    // Check if the model selected delegation
    let delegate_pos = trimmed.find("ROUTE: DELEGATE");
    let direct_pos = trimmed.find("ROUTE: DIRECT");

    match (delegate_pos, direct_pos) {
        (Some(d_idx), None) => parse_delegate_payload(&trimmed[d_idx..]),
        (Some(d_idx), Some(dir_idx)) if d_idx < dir_idx => {
            parse_delegate_payload(&trimmed[d_idx..])
        }
        _ => RoutingDecision::Direct {
            raw_response: trimmed.to_string(),
        },
    }
}

fn parse_delegate_payload(content: &str) -> RoutingDecision {
    let mut plan_summary = String::new();
    let mut tasks = Vec::new();

    // Extract PLAN_SUMMARY if present
    if let Some(plan_start) = content.find("PLAN_SUMMARY:") {
        let after_plan = &content[plan_start + "PLAN_SUMMARY:".len()..];
        let plan_end = after_plan.find("TASKS:").unwrap_or(after_plan.len());
        plan_summary = after_plan[..plan_end].trim().to_string();
    }

    // Extract task blocks
    let tasks_section = if let Some(tasks_start) = content.find("TASKS:") {
        &content[tasks_start + "TASKS:".len()..]
    } else {
        content
    };

    let task_blocks = tasks_section.split("---");
    for block in task_blocks {
        let block_trimmed = block.trim();
        if block_trimmed.is_empty() || !block_trimmed.contains("TASK_NAME:") {
            continue;
        }

        let name = extract_field_value(block_trimmed, "TASK_NAME:");
        let complexity_str = extract_field_value(block_trimmed, "COMPLEXITY:");
        let relevant_context = extract_multiline_field(block_trimmed, "RELEVANT_CONTEXT:", &[
            "TASK:",
            "CONSTRAINTS:",
            "ACCEPTANCE_TESTS:",
        ]);
        let task = extract_multiline_field(block_trimmed, "TASK:", &[
            "CONSTRAINTS:",
            "ACCEPTANCE_TESTS:",
        ]);
        let constraints =
            extract_multiline_field(block_trimmed, "CONSTRAINTS:", &["ACCEPTANCE_TESTS:"]);
        let acceptance_tests = extract_multiline_field(block_trimmed, "ACCEPTANCE_TESTS:", &[]);

        if !name.is_empty() && !task.is_empty() {
            tasks.push(Subtask {
                name,
                complexity: WorkerComplexity::parse_str(&complexity_str),
                relevant_context,
                task,
                constraints,
                acceptance_tests,
            });
        }
    }

    if tasks.is_empty() {
        // Fallback: If DELEGATE was specified but tasks could not be parsed, default to Direct
        RoutingDecision::Direct {
            raw_response: content.to_string(),
        }
    } else {
        RoutingDecision::Delegate {
            plan_summary,
            tasks,
        }
    }
}

fn extract_field_value(content: &str, field_key: &str) -> String {
    let Some(idx) = content.find(field_key) else {
        return String::new();
    };
    let after_key = &content[idx + field_key.len()..];
    let end_idx = after_key.find('\n').unwrap_or(after_key.len());
    after_key[..end_idx].trim().to_string()
}

fn extract_multiline_field(content: &str, field_key: &str, next_keys: &[&str]) -> String {
    let Some(idx) = content.find(field_key) else {
        return String::new();
    };
    let after_key = &content[idx + field_key.len()..];
    let mut min_end = after_key.len();

    for key in next_keys {
        if let Some(key_idx) = after_key.find(key)
            && key_idx < min_end
        {
            min_end = key_idx;
        }
    }

    after_key[..min_end].trim().to_string()
}
