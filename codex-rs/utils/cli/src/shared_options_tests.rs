//! Tests shared command-line option inheritance.

use super::SharedCliOptions;

#[test]
fn inherits_worktree_from_root_without_clearing_subcommand_choice() {
    let mut options = SharedCliOptions::default();
    let root = SharedCliOptions {
        worktree: true,
        ..Default::default()
    };
    options.inherit_exec_root_options(&root);
    assert!(options.worktree);

    options.inherit_exec_root_options(&SharedCliOptions::default());
    assert!(options.worktree);
}

#[test]
fn applies_worktree_subcommand_override_without_clearing_root_choice() {
    let mut options = SharedCliOptions::default();
    options.apply_subcommand_overrides(SharedCliOptions {
        worktree: true,
        ..Default::default()
    });
    assert!(options.worktree);

    options.apply_subcommand_overrides(SharedCliOptions::default());
    assert!(options.worktree);
}

#[test]
fn pipeline_adaptive_override_sets_feature() {
    use super::PipelineModeCliArg;
    use crate::CliConfigOverrides;

    let mut options = SharedCliOptions {
        pipeline: Some(PipelineModeCliArg::Adaptive),
        ..Default::default()
    };
    let mut overrides = CliConfigOverrides::default();
    options.take_auto_review_config_overrides(&mut overrides);

    assert_eq!(
        overrides.raw_overrides,
        vec!["features.adaptive_pipeline=true".to_string()]
    );
    assert_eq!(options.pipeline, None);
}

#[test]
fn pipeline_off_override_disables_feature() {
    use super::PipelineModeCliArg;
    use crate::CliConfigOverrides;

    let mut options = SharedCliOptions {
        pipeline: Some(PipelineModeCliArg::Off),
        ..Default::default()
    };
    let mut overrides = CliConfigOverrides::default();
    options.take_auto_review_config_overrides(&mut overrides);

    assert_eq!(
        overrides.raw_overrides,
        vec!["features.adaptive_pipeline=false".to_string()]
    );
    assert_eq!(options.pipeline, None);
}

#[test]
fn architect_model_override_sets_config() {
    use crate::CliConfigOverrides;

    let mut options = SharedCliOptions {
        architect_model: Some("gpt-5.6-sol".to_string()),
        ..Default::default()
    };
    let mut overrides = CliConfigOverrides::default();
    options.take_auto_review_config_overrides(&mut overrides);

    assert_eq!(
        overrides.raw_overrides,
        vec!["adaptive_pipeline.architect_model=\"gpt-5.6-sol\"".to_string()]
    );
    assert_eq!(options.architect_model, None);
}

#[test]
fn pipeline_compact_override_sets_config() {
    use super::PipelineCompactCliArg;
    use crate::CliConfigOverrides;

    let mut options = SharedCliOptions {
        pipeline_compact: Some(PipelineCompactCliArg::Always),
        ..Default::default()
    };
    let mut overrides = CliConfigOverrides::default();
    options.take_auto_review_config_overrides(&mut overrides);

    assert_eq!(
        overrides.raw_overrides,
        vec!["adaptive_pipeline.compact_mode=\"always\"".to_string()]
    );
    assert_eq!(options.pipeline_compact, None);
}
