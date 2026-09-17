use super::*;
use pretty_assertions::assert_eq;

#[test]
fn format_thousands_comma_separators() {
    assert_eq!(format_thousands(0), "0");
    assert_eq!(format_thousands(999), "999");
    assert_eq!(format_thousands(1000), "1,000");
    assert_eq!(format_thousands(137482), "137,482");
    assert_eq!(format_thousands(143693), "143,693");
    assert_eq!(format_thousands(-12345), "-12,345");
}

#[test]
fn compact_reduction_percent() {
    let compact = CompactMetrics {
        before_tokens: 143693,
        after_tokens: 18420,
    };
    let percent = compact.reduction_percent();
    assert!(percent > 87.0 && percent < 88.0);
}

#[test]
fn pipeline_metrics_aggregation_and_astra_share() {
    let mut metrics = PipelineMetrics::new();
    metrics.context_builder = Some(PhaseMetrics {
        name: "Context Builder".to_string(),
        model: "gpt-5.6-luna".to_string(),
        input_tokens: 137482,
        output_tokens: 6211,
        cached_input_tokens: 0,
        tool_calls: 15,
        duration_ms: 12000,
    });
    metrics.compact = Some(CompactMetrics {
        before_tokens: 143693,
        after_tokens: 18420,
    });
    metrics.architect = Some(PhaseMetrics {
        name: "Architect".to_string(),
        model: "gpt-6-astra".to_string(),
        input_tokens: 25314,
        output_tokens: 4812,
        cached_input_tokens: 0,
        tool_calls: 3,
        duration_ms: 5000,
    });
    metrics.workers.push(PhaseMetrics {
        name: "Worker 1".to_string(),
        model: "gpt-5.6-terra".to_string(),
        input_tokens: 31881,
        output_tokens: 9120,
        cached_input_tokens: 0,
        tool_calls: 5,
        duration_ms: 8000,
    });

    let total_tokens = metrics.total_tokens();
    assert_eq!(
        total_tokens,
        (137482 + 6211) + (25314 + 4812) + (31881 + 9120)
    );

    let expensive_tokens = metrics.expensive_model_tokens();
    assert_eq!(expensive_tokens, 25314 + 4812);

    let cheap_tokens = metrics.cheap_model_tokens();
    assert_eq!(cheap_tokens, (137482 + 6211) + (31881 + 9120));

    let share = metrics.astra_share_percent();
    // 30126 / (143693 + 30126 + 41001) = 30126 / 214820 = ~14.0%
    assert!(share > 13.5 && share < 15.0);

    let cost = metrics.weighted_cost();
    assert!(cost > 0.0);

    let report = metrics.format_pipeline_stats();
    assert!(report.contains("Luna:"));
    assert!(report.contains("137,482 input"));
    assert!(report.contains("6,211 output"));
    assert!(report.contains("Compact:"));
    assert!(report.contains("143,693 → 18,420"));
    assert!(report.contains("Astra:"));
    assert!(report.contains("25,314 input"));
    assert!(report.contains("4,812 output"));
    assert!(report.contains("Terra workers:"));
    assert!(report.contains("31,881 input"));
    assert!(report.contains("9,120 output"));
    assert!(report.contains("Astra share:"));
}
