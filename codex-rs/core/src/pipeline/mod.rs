//! Adaptive Model Pipeline module.
//!
//! Orchestrates a multi-phase pipeline using cheap models for repository exploration
//! and context building, pre-architect compaction, and high-capability models for architecture
//! and task delegation.

pub mod coordinator;
pub mod metrics;
pub mod prompts;
pub mod routing;

pub use coordinator::PipelineCoordinator;
pub use metrics::CompactMetrics;
pub use metrics::PhaseMetrics;
pub use metrics::PipelineMetrics;
pub use routing::RoutingDecision;
pub use routing::Subtask;
pub use routing::WorkerComplexity;
