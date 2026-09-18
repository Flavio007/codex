//! Session lifecycle, native compaction and context preparation for Adaptive Pipeline V2.

use std::sync::Arc;
use std::time::Instant;

use codex_analytics::CompactionTrigger;
use codex_features::Feature;
use codex_history::InitialHistory;
use codex_history::RolloutItem;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::items::TurnItem;
use codex_protocol::protocol::AgentMessageContentDeltaEvent;
use codex_protocol::protocol::AgentMessageEvent;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ItemCompletedEvent;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::protocol::Submission;
use codex_protocol::protocol::TokenCountEvent;
use codex_protocol::turn_input::TurnInputMode;
use codex_protocol::turn_input::TurnInputRequest;
use codex_protocol::turn_input::TurnInputSubmission;
use codex_protocol::turn_input::TurnStartOptions;
use codex_protocol::user_input::UserInput;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::architect_selector::ScoutAssessment;
use super::metrics::CompactMetrics;
use super::metrics::PhaseMetrics;
use super::prompts;
use crate::codex_delegate::run_codex_thread_interactive;
use crate::compact::compact_session;
use crate::config::AdaptivePipelineConfig;
use crate::config::CompactMode;
use crate::config::Constrained;
use crate::config::HandoffMode;
use crate::session::GitEnrichmentPolicy;
use crate::session::SessionIo;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;

/// Manages an interactive Scout session that is kept alive for evaluation and compaction.
pub(crate) struct ScoutSession {
    pub(crate) session: Arc<Session>,
    pub(crate) io: SessionIo,
    pub(crate) handoff: String,
    pub(crate) metrics: PhaseMetrics,
    pub(crate) assessment: ScoutAssessment,
}

impl ScoutSession {
    /// Creates a snapshot of the session's history as `InitialHistory::Forked`.
    pub(crate) async fn fork_compacted_history(&self) -> InitialHistory {
        let history = self.session.clone_history().await;
        let rollout_items: Vec<RolloutItem> = history
            .annotated_items()
            .iter()
            .cloned()
            .map(RolloutItem::ResponseItem)
            .collect();
        InitialHistory::Forked(rollout_items)
    }

    /// Gracefully sends shutdown to the subagent delegate thread.
    pub async fn shutdown(self) {
        let _ = self
            .io
            .tx_sub
            .send(Submission {
                id: "shutdown".to_string(),
                op: Op::Shutdown {},
                trace: None,
                parent_turn_id: None,
                root_turn_id: None,
            })
            .await;
    }
}

/// Context supplied to the Architect phase.
#[derive(Debug, Clone)]
pub enum ArchitectContext {
    /// Textual handoff summary passed directly in prompt without history inheritance.
    ExplicitHandoff { handoff: String },
    /// Compacted history forked into the Architect's initial history.
    CompactedHistory {
        initial_history: InitialHistory,
        handoff_summary: Option<String>,
    },
}

impl ArchitectContext {
    pub fn is_compacted_history(&self) -> bool {
        matches!(self, Self::CompactedHistory { .. })
    }

    pub fn explicit_handoff_text(&self) -> Option<&str> {
        match self {
            Self::ExplicitHandoff { handoff } => Some(handoff.as_str()),
            Self::CompactedHistory { handoff_summary, .. } => handoff_summary.as_deref(),
        }
    }
}

/// Helper for spawning and managing Scout exploration and compaction.
pub struct PipelineCompactionManager;

impl PipelineCompactionManager {
    /// Spawns the Context Builder / Scout in an interactive thread and waits for turn completion,
    /// keeping the session alive for subsequent compaction.
    pub(crate) async fn spawn_scout(
        parent_session: Arc<Session>,
        parent_ctx: Arc<TurnContext>,
        user_prompt: &str,
        context_model: &str,
        cancel_token: CancellationToken,
    ) -> CodexResult<ScoutSession> {
        let cb_start = Instant::now();
        let mut config = parent_ctx.config.as_ref().clone();
        config.model = Some(context_model.to_string());
        config.base_instructions = Some(prompts::CONTEXT_BUILDER_SYSTEM_PROMPT.to_string());
        config.permissions.approval_policy = Constrained::allow_only(AskForApproval::Never);
        let _ = config.features.disable(Feature::Collab);
        let _ = config.features.disable(Feature::MultiAgentV2);
        let _ = config.features.disable(Feature::AdaptivePipeline);

        let input = vec![UserInput::Text {
            text: prompts::context_builder_user_prompt(user_prompt),
            text_elements: Vec::new(),
        }];

        let parent_environments = parent_ctx.environments.clone();
        let parent_turn_id = parent_ctx.sub_id.clone();
        let root_turn_id = parent_ctx.turn_metadata_state.root_turn_id();

        let (scout_session, io) = Box::pin(run_codex_thread_interactive(
            config,
            Arc::clone(&parent_session.services.auth_manager),
            Arc::clone(&parent_session.services.models_manager),
            Arc::clone(&parent_session),
            parent_ctx.clone(),
            parent_environments,
            cancel_token.child_token(),
            SubAgentSource::ThreadSpawn {
                parent_thread_id: parent_session.thread_id(),
                depth: 1,
                agent_path: None,
                agent_nickname: None,
                agent_role: None,
            },
            codex_extension_api::SessionIsolation::Inherit,
            /*initial_history*/ None,
            GitEnrichmentPolicy::Fresh,
            codex_sandboxing::WindowsSandboxProxySettingsMode::Reconcile,
        ))
        .await?;

        // Submit the initial Scout turn with JSON schema guidance
        let submission = io
            .submit_turn_input(
                TurnInputRequest::user_input(input).on_start(TurnStartOptions {
                    final_output_json_schema: Some(prompts::scout_assessment_json_schema()),
                    service_tier: None,
                    parent_turn_id: Some(parent_turn_id),
                    root_turn_id,
                    ..Default::default()
                }),
                TurnInputMode::StartIfIdle,
            )
            .await?;

        if !matches!(submission, TurnInputSubmission::Started { .. }) {
            return Err(CodexErr::InvalidRequest(format!(
                "scout turn input was not started: {submission:?}"
            )));
        }

        // Drain events until turn completion
        let (raw_output, (in_tokens, out_tokens), tool_calls) =
            drain_scout_events(&io.rx_event).await?;

        let (handoff, assessment) = ScoutAssessment::parse_scout_output(&raw_output);

        let metrics = PhaseMetrics {
            name: "Context Builder".to_string(),
            model: context_model.to_string(),
            input_tokens: in_tokens,
            output_tokens: out_tokens,
            cached_input_tokens: 0,
            tool_calls,
            duration_ms: cb_start.elapsed().as_millis() as u64,
        };

        Ok(ScoutSession {
            session: scout_session,
            io,
            handoff,
            metrics,
            assessment,
        })
    }

    /// Determines whether native compaction should execute.
    pub fn should_compact(config: &AdaptivePipelineConfig, scout_total_tokens: i64) -> bool {
        match config.compact_mode {
            CompactMode::Off => false,
            CompactMode::Always => true,
            CompactMode::Auto => scout_total_tokens >= config.compact_threshold as i64,
        }
    }

    /// Evaluates compaction policy, runs /compact on the Scout session if appropriate,
    /// and constructs the resulting ArchitectContext.
    pub(crate) async fn prepare_architect_context(
        config: &AdaptivePipelineConfig,
        scout: &ScoutSession,
        ctx: Arc<TurnContext>,
    ) -> CodexResult<(ArchitectContext, Option<CompactMetrics>)> {
        let scout_tokens = scout.metrics.total_tokens();
        let do_compact = Self::should_compact(config, scout_tokens);

        if !do_compact {
            info!("Compaction bypassed per compact_mode policy");
            return Ok((
                ArchitectContext::ExplicitHandoff {
                    handoff: scout.handoff.clone(),
                },
                None,
            ));
        }

        info!(
            scout_tokens,
            threshold = config.compact_threshold,
            "Running native /compact on live Scout session"
        );
        let compact_start = Instant::now();

        let outcome =
            compact_session(scout.session.clone(), ctx, CompactionTrigger::Auto).await?;

        let duration_ms = compact_start.elapsed().as_millis() as u64;
        let compact_metrics = CompactMetrics {
            mode: format!("{:?}", config.compact_mode).to_lowercase(),
            before_tokens: outcome.tokens_before,
            after_tokens: outcome.tokens_after,
            duration_ms,
            implementation: format!("{:?}", outcome.implementation),
        };

        // If handoff_mode is Compact, fork the session's newly compacted history for the Architect.
        // If Summary, pass the explicit text handoff (or compaction summary if non-empty).
        let architect_context = match config.handoff_mode {
            HandoffMode::Compact => {
                let initial_history = scout.fork_compacted_history().await;
                let summary_text = if !outcome.summary.is_empty() {
                    Some(outcome.summary)
                } else {
                    Some(scout.handoff.clone())
                };
                ArchitectContext::CompactedHistory {
                    initial_history,
                    handoff_summary: summary_text,
                }
            }
            HandoffMode::Summary => {
                let handoff_text = if !outcome.summary.is_empty() {
                    outcome.summary
                } else {
                    scout.handoff.clone()
                };
                ArchitectContext::ExplicitHandoff {
                    handoff: handoff_text,
                }
            }
        };

        Ok((architect_context, Some(compact_metrics)))
    }
}

async fn drain_scout_events(
    receiver: &async_channel::Receiver<Event>,
) -> CodexResult<(String, (i64, i64), usize)> {
    let mut collected_text = String::new();
    let mut input_tokens = 0i64;
    let mut output_tokens = 0i64;
    let mut tool_calls = 0usize;

    while let Ok(event) = receiver.recv().await {
        let is_terminal = matches!(
            event.msg,
            EventMsg::TurnComplete(_) | EventMsg::TurnAborted(_)
        );
        match event.msg {
            EventMsg::AgentMessage(AgentMessageEvent { message, .. }) => {
                collected_text = message;
            }
            EventMsg::AgentMessageContentDelta(AgentMessageContentDeltaEvent { delta, .. }) => {
                collected_text.push_str(&delta);
            }
            EventMsg::ItemCompleted(ItemCompletedEvent { item, .. }) => {
                if matches!(
                    item,
                    TurnItem::CommandExecution(_)
                        | TurnItem::DynamicToolCall(_)
                        | TurnItem::CollabAgentToolCall(_)
                        | TurnItem::FunctionCallOutput(_)
                ) {
                    tool_calls = tool_calls.saturating_add(1);
                }
            }
            EventMsg::TokenCount(TokenCountEvent {
                info: Some(info), ..
            }) => {
                input_tokens = input_tokens.max(info.total_token_usage.input_tokens);
                output_tokens = output_tokens.max(info.total_token_usage.output_tokens);
            }
            _ => {}
        }
        if is_terminal {
            break;
        }
    }

    if input_tokens == 0 && output_tokens == 0 && !collected_text.is_empty() {
        output_tokens = (collected_text.len() / 4) as i64;
        input_tokens = output_tokens.saturating_mul(4);
    }

    Ok((collected_text, (input_tokens, output_tokens), tool_calls))
}
