//! Cost measurement, token tracking, and comparative stats for the Adaptive Model Pipeline.

use serde::Deserialize;
use serde::Serialize;

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod tests;

/// Token prices per million tokens (USD).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ModelPricePerMillion {
    pub input: f64,
    pub output: f64,
}

impl ModelPricePerMillion {
    pub const fn new(input: f64, output: f64) -> Self {
        Self { input, output }
    }

    pub fn compute_cost(self, input_tokens: i64, output_tokens: i64) -> f64 {
        let in_cost = (input_tokens as f64 / 1_000_000.0) * self.input;
        let out_cost = (output_tokens as f64 / 1_000_000.0) * self.output;
        in_cost + out_cost
    }
}

/// Known model pricing catalog for calculating weighted pipeline cost.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PipelinePricingCatalog {
    pub luna: ModelPricePerMillion,
    pub terra: ModelPricePerMillion,
    pub sol: ModelPricePerMillion,
    pub astra: ModelPricePerMillion,
}

impl Default for PipelinePricingCatalog {
    fn default() -> Self {
        Self {
            // Low-cost context exploration tier (e.g. gpt-5.6-luna)
            luna: ModelPricePerMillion::new(0.15, 0.60),
            // Standard worker tier (e.g. gpt-5.6-terra)
            terra: ModelPricePerMillion::new(0.50, 2.00),
            // High-complexity worker tier (e.g. gpt-5.6-sol)
            sol: ModelPricePerMillion::new(1.50, 6.00),
            // Flagship reasoning / architecture tier (e.g. gpt-6-astra)
            astra: ModelPricePerMillion::new(5.00, 20.00),
        }
    }
}

impl PipelinePricingCatalog {
    pub fn price_for_model(&self, model: &str) -> ModelPricePerMillion {
        let m = model.to_lowercase();
        if m.contains("luna") || m.contains("mini") || m.contains("flash") {
            self.luna
        } else if m.contains("terra") || m.contains("standard") {
            self.terra
        } else if m.contains("sol") || m.contains("plus") {
            self.sol
        } else {
            // Default expensive flagship
            self.astra
        }
    }
}

/// Token metrics collected for a single pipeline phase.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseMetrics {
    pub name: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub tool_calls: usize,
    pub duration_ms: u64,
}

impl PhaseMetrics {
    pub fn total_tokens(&self) -> i64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

/// Token metrics for the pre-architect compaction step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactMetrics {
    pub before_tokens: i64,
    pub after_tokens: i64,
}

impl CompactMetrics {
    pub fn reduction_percent(&self) -> f64 {
        if self.before_tokens <= 0 {
            return 0.0;
        }
        let reduction = self.before_tokens.saturating_sub(self.after_tokens);
        (reduction as f64 / self.before_tokens as f64) * 100.0
    }
}

/// Consolidated metrics across the entire adaptive pipeline execution.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PipelineMetrics {
    pub context_builder: Option<PhaseMetrics>,
    pub compact: Option<CompactMetrics>,
    pub architect: Option<PhaseMetrics>,
    pub workers: Vec<PhaseMetrics>,
    pub total_wall_time_ms: u64,
    pub total_tool_calls: usize,
    pub pricing: PipelinePricingCatalog,
}

impl PipelineMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn total_input_tokens(&self) -> i64 {
        let cb = self.context_builder.as_ref().map_or(0, |m| m.input_tokens);
        let ar = self.architect.as_ref().map_or(0, |m| m.input_tokens);
        let wk: i64 = self.workers.iter().map(|w| w.input_tokens).sum();
        cb + ar + wk
    }

    pub fn total_output_tokens(&self) -> i64 {
        let cb = self.context_builder.as_ref().map_or(0, |m| m.output_tokens);
        let ar = self.architect.as_ref().map_or(0, |m| m.output_tokens);
        let wk: i64 = self.workers.iter().map(|w| w.output_tokens).sum();
        cb + ar + wk
    }

    pub fn total_tokens(&self) -> i64 {
        self.total_input_tokens() + self.total_output_tokens()
    }

    /// Tokens processed by expensive flagship model (Architect / Astra).
    pub fn expensive_model_tokens(&self) -> i64 {
        self.architect.as_ref().map_or(0, PhaseMetrics::total_tokens)
    }

    /// Tokens processed by cheap exploration and worker models (Luna/Terra/Sol).
    pub fn cheap_model_tokens(&self) -> i64 {
        let cb = self.context_builder.as_ref().map_or(0, PhaseMetrics::total_tokens);
        let wk: i64 = self.workers.iter().map(PhaseMetrics::total_tokens).sum();
        cb + wk
    }

    /// Percentage of total tokens processed by Astra (flagship model).
    pub fn astra_share_percent(&self) -> f64 {
        let total = self.total_tokens();
        if total <= 0 {
            return 0.0;
        }
        (self.expensive_model_tokens() as f64 / total as f64) * 100.0
    }

    /// Computes total weighted cost using model price rates.
    pub fn weighted_cost(&self) -> f64 {
        let mut total = 0.0;
        if let Some(cb) = &self.context_builder {
            let price = self.pricing.price_for_model(&cb.model);
            total += price.compute_cost(cb.input_tokens, cb.output_tokens);
        }
        if let Some(ar) = &self.architect {
            let price = self.pricing.price_for_model(&ar.model);
            total += price.compute_cost(ar.input_tokens, ar.output_tokens);
        }
        for w in &self.workers {
            let price = self.pricing.price_for_model(&w.model);
            total += price.compute_cost(w.input_tokens, w.output_tokens);
        }
        total
    }

    /// Formats the summary output matching the requested specification.
    pub fn format_pipeline_stats(&self) -> String {
        let mut out = String::new();
        out.push_str("Pipeline stats\n\n");

        if let Some(cb) = &self.context_builder {
            let label = if cb.model.is_empty() {
                "Luna"
            } else {
                model_display_name(&cb.model)
            };
            out.push_str(&format!("{label}:\n"));
            out.push_str(&format!("  {} input\n", format_thousands(cb.input_tokens)));
            out.push_str(&format!("  {} output\n\n", format_thousands(cb.output_tokens)));
        }

        if let Some(compact) = &self.compact {
            out.push_str("Compact:\n");
            out.push_str(&format!(
                "  {} → {}\n\n",
                format_thousands(compact.before_tokens),
                format_thousands(compact.after_tokens)
            ));
        }

        if let Some(ar) = &self.architect {
            let label = if ar.model.is_empty() {
                "Astra"
            } else {
                model_display_name(&ar.model)
            };
            out.push_str(&format!("{label}:\n"));
            out.push_str(&format!("  {} input\n", format_thousands(ar.input_tokens)));
            out.push_str(&format!("  {} output\n\n", format_thousands(ar.output_tokens)));
        }

        if !self.workers.is_empty() {
            let worker_in: i64 = self.workers.iter().map(|w| w.input_tokens).sum();
            let worker_out: i64 = self.workers.iter().map(|w| w.output_tokens).sum();
            out.push_str("Terra workers:\n");
            out.push_str(&format!("  {} input\n", format_thousands(worker_in)));
            out.push_str(&format!("  {} output\n\n", format_thousands(worker_out)));
        }

        out.push_str(&format!(
            "Astra share:\n  {:.1}% of total processed tokens\n",
            self.astra_share_percent()
        ));

        out.push_str(&format!(
            "\nEstimated weighted cost: ${:.4}\n",
            self.weighted_cost()
        ));

        out
    }
}

/// Helper to format large integers with commas (e.g. 137482 -> 137,482).
pub fn format_thousands(n: i64) -> String {
    let s = n.to_string();
    let is_negative = s.starts_with('-');
    let digits = if is_negative { &s[1..] } else { &s };

    let mut result = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    let rem = digits.len() % 3;

    if rem > 0 {
        result.push_str(&digits[..rem]);
    }
    for (i, chunk) in digits[rem..].as_bytes().chunks(3).enumerate() {
        if rem > 0 || i > 0 {
            result.push(',');
        }
        result.push_str(std::str::from_utf8(chunk).unwrap_or(""));
    }

    if is_negative {
        format!("-{result}")
    } else {
        result
    }
}

fn model_display_name(slug: &str) -> &str {
    if slug.contains("luna") {
        "Luna"
    } else if slug.contains("astra") {
        "Astra"
    } else if slug.contains("terra") {
        "Terra"
    } else if slug.contains("sol") {
        "Sol"
    } else {
        slug
    }
}
