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
        mode: "auto".to_string(),
        before_tokens: 143693,
        after_tokens: 18420,
        duration_ms: 3200,
        implementation: "remote_v2".to_string(),
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
        cached_input_tokens: 94331,
        tool_calls: 15,
        duration_ms: 12000,
    });
    metrics.compact = Some(CompactMetrics {
        mode: "auto".to_string(),
        before_tokens: 143693,
        after_tokens: 18420,
        duration_ms: 3200,
        implementation: "remote_v2".to_string(),
    });
    metrics.architect_selection = Some(ArchitectSelectionMetrics {
        mode: "auto".to_string(),
        score: Some(5),
        initially_selected: "gpt-5.6-terra".to_string(),
        final_model: "gpt-5.6-terra".to_string(),
        escalations: 0,
    });
    metrics.architect = Some(PhaseMetrics {
        name: "Architect".to_string(),
        model: "gpt-6-astra".to_string(),
        input_tokens: 25314,
        output_tokens: 4812,
        cached_input_tokens: 4821,
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

    let share = metrics.astra_share_percent();
    assert!(share > 13.5 && share < 15.0);

    let cost = metrics.weighted_cost();
    assert!(cost > 0.0);

    let astra_cost = metrics.equivalent_astra_cost();
    assert!(astra_cost > cost);

    let report = metrics.format_pipeline_stats();
    assert!(report.contains("Adaptive Pipeline Stats"));
    assert!(report.contains("Scout"));
    assert!(report.contains("Luna"));
    assert!(report.contains("137,482"));
    assert!(report.contains("Compaction"));
    assert!(report.contains("87.2%"));
    assert!(report.contains("Architect selection"));
    assert!(report.contains("Terra"));
    assert!(report.contains("Workers"));
    assert!(report.contains("Terra: 1"));
    assert!(report.contains("equivalent Astra cost:"));
}
