//! Adaptive Model Pipeline module.
//!
//! Orchestrates a multi-phase pipeline using cheap models for repository exploration
//! and context building, pre-architect compaction, and high-capability models for architecture
//! and task delegation.

pub mod architect_selector;
pub mod compaction;
pub mod coordinator;
pub mod metrics;
pub mod prompts;
pub mod routing;

pub use architect_selector::ArchitectSelection;
pub use architect_selector::ArchitectSelector;
pub use architect_selector::ArchitectTier;
pub use architect_selector::ScoutAssessment;
pub use compaction::ArchitectContext;
pub(crate) use coordinator::PipelineCoordinator;
pub use metrics::CompactMetrics;
pub use metrics::PhaseMetrics;
pub use metrics::PipelineMetrics;
pub use routing::ArchitectStatus;
pub use routing::RoutingDecision;
pub use routing::Subtask;
pub use routing::WorkerComplexity;

