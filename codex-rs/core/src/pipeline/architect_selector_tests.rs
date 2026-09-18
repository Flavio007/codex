use super::*;
use pretty_assertions::assert_eq;

#[test]
fn explicit_model_bypasses_selector() {
    let mut config = AdaptivePipelineConfig::default();
    config.architect_model = "gpt-5.6-sol".to_string();

    let assessment = ScoutAssessment {
        estimated_files: 10,
        estimated_subsystems: 5,
        concurrency_or_unsafe: true,
        architectural_change: true,
        cross_cutting: true,
        ambiguity: AmbiguityLevel::High,
        ..Default::default()
    };

    let selection = ArchitectSelector::select(&config, &assessment);
    assert_eq!(
        selection,
        ArchitectSelection::Explicit("gpt-5.6-sol".to_string())
    );
    assert_eq!(selection.model(), "gpt-5.6-sol");
    assert_eq!(selection.tier(), Some(ArchitectTier::Sol));
}

#[test]
fn score_computation_and_tier_mapping() {
    // Low score -> Luna
    let assessment_luna = ScoutAssessment {
        estimated_files: 2,
        estimated_subsystems: 1,
        estimated_tasks: 1,
        ambiguity: AmbiguityLevel::Low,
        ..Default::default()
    };
    let (score_luna, _) = ArchitectSelector::compute_score(&assessment_luna);
    assert_eq!(score_luna, 0);

    let config = AdaptivePipelineConfig::default(); // architect_model = "auto"
    let sel_luna = ArchitectSelector::select(&config, &assessment_luna);
    assert_eq!(sel_luna.tier(), Some(ArchitectTier::Luna));
    assert_eq!(sel_luna.model(), "gpt-5.6-luna");

    // Medium score -> Terra
    let assessment_terra = ScoutAssessment {
        estimated_files: 8, // +1
        estimated_subsystems: 3, // +2
        estimated_tasks: 4, // +1
        ambiguity: AmbiguityLevel::Low,
        ..Default::default()
    };
    let (score_terra, _) = ArchitectSelector::compute_score(&assessment_terra);
    assert_eq!(score_terra, 4);
    let sel_terra = ArchitectSelector::select(&config, &assessment_terra);
    assert_eq!(sel_terra.tier(), Some(ArchitectTier::Terra));
    assert_eq!(sel_terra.model(), "gpt-5.6-terra");

    // High score -> Sol
    let assessment_sol = ScoutAssessment {
        estimated_files: 8, // +1
        estimated_subsystems: 3, // +2
        estimated_tasks: 4, // +1
        performance_sensitive: true, // +2
        public_api_change: true, // +1
        ambiguity: AmbiguityLevel::Medium, // +1
        ..Default::default()
    };
    let (score_sol, _) = ArchitectSelector::compute_score(&assessment_sol);
    assert_eq!(score_sol, 7);
    let sel_sol = ArchitectSelector::select(&config, &assessment_sol);
    assert_eq!(sel_sol.tier(), Some(ArchitectTier::Sol));
    assert_eq!(sel_sol.model(), "gpt-5.6-sol");
}

#[test]
fn floor_rule_concurrency_or_unsafe_enforces_at_least_sol() {
    let config = AdaptivePipelineConfig::default();
    // Task with low score (score = 2) but concurrency_or_unsafe is true
    let assessment = ScoutAssessment {
        concurrency_or_unsafe: true, // +2
        estimated_files: 1,
        estimated_subsystems: 1,
        ambiguity: AmbiguityLevel::Low,
        ..Default::default()
    };

    let (score, _) = ArchitectSelector::compute_score(&assessment);
    assert_eq!(score, 2); // Score alone would be Luna

    let selection = ArchitectSelector::select(&config, &assessment);
    assert_eq!(selection.tier(), Some(ArchitectTier::Sol));
    assert_eq!(selection.model(), "gpt-5.6-sol");
}

#[test]
fn floor_rule_architectural_cross_cutting_high_ambiguity_enforces_astra() {
    let config = AdaptivePipelineConfig::default();
    let assessment = ScoutAssessment {
        architectural_change: true, // +2
        cross_cutting: true, // +2
        ambiguity: AmbiguityLevel::High, // +3
        estimated_files: 2,
        estimated_subsystems: 1,
        estimated_tasks: 1,
        ..Default::default()
    };

    let (score, _) = ArchitectSelector::compute_score(&assessment);
    assert_eq!(score, 7); // Score alone (7) would map to Sol

    let selection = ArchitectSelector::select(&config, &assessment);
    assert_eq!(selection.tier(), Some(ArchitectTier::Astra));
    assert_eq!(selection.model(), "gpt-6-astra");
}

#[test]
fn clamping_auto_max_prevents_astra() {
    let mut config = AdaptivePipelineConfig::default();
    config.architect_auto_max = "gpt-5.6-sol".to_string();

    let assessment = ScoutAssessment {
        architectural_change: true,
        cross_cutting: true,
        ambiguity: AmbiguityLevel::High,
        ..Default::default()
    };

    let selection = ArchitectSelector::select(&config, &assessment);
    assert_eq!(selection.tier(), Some(ArchitectTier::Sol));
    assert_eq!(selection.model(), "gpt-5.6-sol");
}

#[test]
fn parse_scout_output_json_and_markdown() {
    // 1. Direct JSON
    let json_text = r#"{
        "handoff": "# HANDOFF_SUMMARY\nScope: refactor parser",
        "assessment": {
            "estimated_files": 3,
            "estimated_subsystems": 2,
            "estimated_tasks": 2,
            "cross_cutting": false,
            "architectural_change": false,
            "concurrency_or_unsafe": false,
            "performance_sensitive": false,
            "public_api_change": false,
            "migration_required": false,
            "ambiguity": "low"
        }
    }"#;

    let (handoff, assessment) = ScoutAssessment::parse_scout_output(json_text);
    assert_eq!(handoff, "# HANDOFF_SUMMARY\nScope: refactor parser");
    assert_eq!(assessment.estimated_files, 3);
    assert_eq!(assessment.estimated_subsystems, 2);
    assert_eq!(assessment.ambiguity, AmbiguityLevel::Low);

    // 2. Fenced ```json block
    let md_text = r#"
Investigação concluída com sucesso.

```json
{
    "handoff": "# HANDOFF_SUMMARY\nDetailed handoff findings",
    "assessment": {
        "estimated_files": 5,
        "concurrency_or_unsafe": true,
        "ambiguity": "medium"
    }
}
```
"#;

    let (handoff2, assessment2) = ScoutAssessment::parse_scout_output(md_text);
    assert_eq!(handoff2, "# HANDOFF_SUMMARY\nDetailed handoff findings");
    assert_eq!(assessment2.estimated_files, 5);
    assert_eq!(assessment2.concurrency_or_unsafe, true);
    assert_eq!(assessment2.ambiguity, AmbiguityLevel::Medium);

    // 3. Fallback plaintext
    let raw_text = "Algum texto preliminar...\n\n# HANDOFF_SUMMARY\nRequisitos e escopo puro.";
    let (handoff3, assessment3) = ScoutAssessment::parse_scout_output(raw_text);
    assert_eq!(handoff3, "# HANDOFF_SUMMARY\nRequisitos e escopo puro.");
    assert_eq!(assessment3, ScoutAssessment::default());
}
