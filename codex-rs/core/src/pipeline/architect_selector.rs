//! Architecture model auto-selector and complexity assessment for Adaptive Pipeline V2.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::config::AdaptivePipelineConfig;

#[cfg(test)]
#[path = "architect_selector_tests.rs"]
mod tests;

/// Ambiguity level of the problem requirements or scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AmbiguityLevel {
    #[default]
    Low,
    Medium,
    High,
}

impl AmbiguityLevel {
    pub fn parse_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "high" | "alto" | "alta" => Self::High,
            "medium" | "medio" | "media" | "moderate" => Self::Medium,
            _ => Self::Low,
        }
    }
}

/// Structured complexity assessment produced by the Scout before handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoutAssessment {
    #[serde(default)]
    pub estimated_files: usize,
    #[serde(default)]
    pub estimated_subsystems: usize,
    #[serde(default)]
    pub estimated_tasks: usize,

    #[serde(default)]
    pub cross_cutting: bool,
    #[serde(default)]
    pub architectural_change: bool,
    #[serde(default)]
    pub concurrency_or_unsafe: bool,
    #[serde(default)]
    pub performance_sensitive: bool,
    #[serde(default)]
    pub public_api_change: bool,
    #[serde(default)]
    pub migration_required: bool,

    #[serde(default)]
    pub ambiguity: AmbiguityLevel,
}

impl Default for ScoutAssessment {
    fn default() -> Self {
        Self {
            estimated_files: 1,
            estimated_subsystems: 1,
            estimated_tasks: 1,
            cross_cutting: false,
            architectural_change: false,
            concurrency_or_unsafe: false,
            performance_sensitive: false,
            public_api_change: false,
            migration_required: false,
            ambiguity: AmbiguityLevel::Low,
        }
    }
}

impl ScoutAssessment {
    /// Parses output from the scout. Expects either JSON with `{"handoff": "...", "assessment": {...}}`,
    /// markdown with embedded ```json block, or falls back to raw text and default assessment.
    pub fn parse_scout_output(raw_output: &str) -> (String, Self) {
        let trimmed = raw_output.trim();

        // 1. Try direct JSON parse
        if let Ok(val) = serde_json::from_str::<Value>(trimmed)
            && let Some(obj) = val.as_object()
        {
            let handoff = obj
                .get("handoff")
                .and_then(|h| h.as_str())
                .unwrap_or(trimmed)
                .to_string();
            let assessment = obj
                .get("assessment")
                .and_then(|a| serde_json::from_value::<Self>(a.clone()).ok())
                .unwrap_or_default();
            return (handoff, assessment);
        }

        // 2. Try fenced ```json ``` block
        if let Some(json_start) = trimmed.find("```json") {
            let after_start = &trimmed[json_start + 7..];
            if let Some(json_end) = after_start.find("```") {
                let json_slice = after_start[..json_end].trim();
                if let Ok(val) = serde_json::from_str::<Value>(json_slice)
                    && let Some(obj) = val.as_object()
                {
                    let handoff = obj
                        .get("handoff")
                        .and_then(|h| h.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            // If handoff was outside the json block, use preceding or following text
                            extract_summary_or_raw(trimmed)
                        });
                    let assessment = obj
                        .get("assessment")
                        .and_then(|a| serde_json::from_value::<Self>(a.clone()).ok())
                        .unwrap_or_default();
                    return (handoff, assessment);
                }
            }
        }

        // 3. Fallback: extract # HANDOFF_SUMMARY if present
        (extract_summary_or_raw(trimmed), Self::default())
    }

    /// Formats a concise summary of complexity signals for injection into prompts.
    pub fn format_summary(&self) -> String {
        format!(
            "- Estimated relevant files: {}\n\
             - Estimated subsystems affected: {}\n\
             - Estimated subtasks: {}\n\
             - Cross-cutting impact: {}\n\
             - Architectural change: {}\n\
             - Concurrency or unsafe code: {}\n\
             - Performance sensitive: {}\n\
             - Public API change: {}\n\
             - Migration required: {}\n\
             - Ambiguity: {:?}",
            self.estimated_files,
            self.estimated_subsystems,
            self.estimated_tasks,
            self.cross_cutting,
            self.architectural_change,
            self.concurrency_or_unsafe,
            self.performance_sensitive,
            self.public_api_change,
            self.migration_required,
            self.ambiguity,
        )
    }
}

fn extract_summary_or_raw(s: &str) -> String {
    if let Some(idx) = s.find("# HANDOFF_SUMMARY") {
        s[idx..].trim().to_string()
    } else {
        s.to_string()
    }
}

/// Tier ranking for candidate Architect models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchitectTier {
    Luna = 0,
    Terra = 1,
    Sol = 2,
    Astra = 3,
}

impl ArchitectTier {
    pub fn model_name(self) -> &'static str {
        match self {
            Self::Luna => "gpt-5.6-luna",
            Self::Terra => "gpt-5.6-terra",
            Self::Sol => "gpt-5.6-sol",
            Self::Astra => "gpt-6-astra",
        }
    }

    pub fn from_model_name(name: &str) -> Option<Self> {
        let lower = name.to_lowercase();
        if lower.contains("luna") || lower.contains("mini") || lower.contains("flash") {
            Some(Self::Luna)
        } else if lower.contains("terra") || lower.contains("standard") {
            Some(Self::Terra)
        } else if lower.contains("sol") || lower.contains("plus") {
            Some(Self::Sol)
        } else if lower.contains("astra") || lower.contains("flagship") {
            Some(Self::Astra)
        } else {
            None
        }
    }

    pub fn next_tier(self) -> Self {
        match self {
            Self::Luna => Self::Terra,
            Self::Terra => Self::Sol,
            Self::Sol | Self::Astra => Self::Astra,
        }
    }
}

/// Result of evaluating candidate Architect models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArchitectSelection {
    Explicit(String),
    Auto {
        model: String,
        tier: ArchitectTier,
        score: u32,
        reasons: Vec<String>,
    },
}

impl ArchitectSelection {
    pub fn model(&self) -> &str {
        match self {
            Self::Explicit(m) => m.as_str(),
            Self::Auto { model, .. } => model.as_str(),
        }
    }

    pub fn tier(&self) -> Option<ArchitectTier> {
        match self {
            Self::Explicit(m) => ArchitectTier::from_model_name(m),
            Self::Auto { tier, .. } => Some(*tier),
        }
    }
}

/// Evaluates ScoutAssessment and configuration to select the optimal Architect model.
pub struct ArchitectSelector;

impl ArchitectSelector {
    /// Selects the architect model. If `config.architect_model != "auto"`, bypasses evaluation.
    pub fn select(
        config: &AdaptivePipelineConfig,
        assessment: &ScoutAssessment,
    ) -> ArchitectSelection {
        if config.architect_model != "auto" {
            return ArchitectSelection::Explicit(config.architect_model.clone());
        }

        let (score, reasons) = Self::compute_score(assessment);
        let mut reasons = reasons;

        // Base tier from score: 0-2 -> Luna, 3-5 -> Terra, 6-8 -> Sol, 9+ -> Astra
        let mut tier = match score {
            0..=2 => ArchitectTier::Luna,
            3..=5 => ArchitectTier::Terra,
            6..=8 => ArchitectTier::Sol,
            _ => ArchitectTier::Astra,
        };

        // Hard rule 1: Concurrency or unsafe code imposes a floor of Sol
        if assessment.concurrency_or_unsafe && tier < ArchitectTier::Sol {
            tier = ArchitectTier::Sol;
            reasons.push("floor rule: concurrency/unsafe requires at least Sol".to_string());
        }

        // Hard rule 2: Architectural change with cross-cutting impact and high ambiguity requires Astra
        if assessment.architectural_change
            && assessment.cross_cutting
            && assessment.ambiguity == AmbiguityLevel::High
        {
            tier = ArchitectTier::Astra;
            reasons.push(
                "floor rule: cross-cutting architectural change with high ambiguity requires Astra"
                    .to_string(),
            );
        }

        // Apply configured bounds [architect_auto_min, architect_auto_max]
        let min_tier = ArchitectTier::from_model_name(&config.architect_auto_min)
            .unwrap_or(ArchitectTier::Luna);
        let max_tier = ArchitectTier::from_model_name(&config.architect_auto_max)
            .unwrap_or(ArchitectTier::Astra);

        let clamped_tier = tier.clamp(min_tier, max_tier);
        if clamped_tier != tier {
            reasons.push(format!("clamped to [{min_tier:?}..={max_tier:?}]"));
            tier = clamped_tier;
        }

        ArchitectSelection::Auto {
            model: tier.model_name().to_string(),
            tier,
            score,
            reasons,
        }
    }

    /// Computes deterministic complexity score and records individual point additions.
    pub fn compute_score(assessment: &ScoutAssessment) -> (u32, Vec<String>) {
        let mut score = 0u32;
        let mut reasons = Vec::new();

        if assessment.architectural_change {
            score = score.saturating_add(2);
            reasons.push("+2 architectural_change".to_string());
        }
        if assessment.cross_cutting {
            score = score.saturating_add(2);
            reasons.push("+2 cross_cutting".to_string());
        }
        if assessment.concurrency_or_unsafe {
            score = score.saturating_add(2);
            reasons.push("+2 concurrency_or_unsafe".to_string());
        }
        if assessment.performance_sensitive {
            score = score.saturating_add(2);
            reasons.push("+2 performance_sensitive".to_string());
        }
        if assessment.public_api_change {
            score = score.saturating_add(1);
            reasons.push("+1 public_api_change".to_string());
        }
        if assessment.migration_required {
            score = score.saturating_add(1);
            reasons.push("+1 migration_required".to_string());
        }

        if assessment.estimated_subsystems >= 3 {
            score = score.saturating_add(2);
            reasons.push("+2 estimated_subsystems >= 3".to_string());
        }
        if assessment.estimated_files >= 8 {
            score = score.saturating_add(1);
            reasons.push("+1 estimated_files >= 8".to_string());
        }
        if assessment.estimated_tasks >= 4 {
            score = score.saturating_add(1);
            reasons.push("+1 estimated_tasks >= 4".to_string());
        }

        match assessment.ambiguity {
            AmbiguityLevel::Low => {}
            AmbiguityLevel::Medium => {
                score = score.saturating_add(1);
                reasons.push("+1 medium ambiguity".to_string());
            }
            AmbiguityLevel::High => {
                score = score.saturating_add(3);
                reasons.push("+3 high ambiguity".to_string());
            }
        }

        (score, reasons)
    }
}
