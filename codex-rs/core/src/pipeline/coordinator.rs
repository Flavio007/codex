//! Core coordinator for the Adaptive Model Pipeline.

use std::sync::Arc;
use std::time::Instant;

use codex_features::Feature;
use codex_protocol::error::CodexResult;
use codex_protocol::protocol::AgentMessageContentDeltaEvent;
use codex_protocol::protocol::AgentMessageEvent;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ItemCompletedEvent;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::protocol::TokenCountEvent;
use codex_protocol::user_input::UserInput;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::metrics::CompactMetrics;
use super::metrics::PhaseMetrics;
use super::metrics::PipelineMetrics;
use super::prompts;
use super::routing;
use super::routing::RoutingDecision;
use crate::codex_delegate::run_codex_thread_one_shot;
use crate::config::Constrained;
use crate::session::TurnInput;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;

pub struct PipelineCoordinator;

impl PipelineCoordinator {
    /// Executes the full adaptive pipeline flow.
    #[tracing::instrument(name = "adaptive_pipeline.run", skip_all)]
    pub async fn run(
        session: Arc<Session>,
        ctx: Arc<TurnContext>,
        input: Vec<TurnInput>,
        cancellation_token: CancellationToken,
    ) -> CodexResult<Option<String>> {
        let pipeline_start = Instant::now();
        let pipeline_cfg = ctx.config.adaptive_pipeline.clone();

        let mut user_prompt = String::new();
        for item in input {
            if let TurnInput::UserInput { content, .. } = item {
                for user_item in content {
                    if let UserInput::Text { text, .. } = user_item {
                        if !user_prompt.is_empty() {
                            user_prompt.push('\n');
                        }
                        user_prompt.push_str(&text);
                    }
                }
            }
        }

        if user_prompt.trim().is_empty() {
            user_prompt = "Execute o plano de trabalho no repositório.".to_string();
        }

        let mut metrics = PipelineMetrics::new();

        // -------------------------------------------------------------
        // Phase 1: Context Builder (Luna / Cheap Model)
        // -------------------------------------------------------------
        info!(
            model = %pipeline_cfg.context_model,
            "Starting Phase 1: Context Builder"
        );
        emit_pipeline_status(
            &session,
            &ctx,
            &format!(
                "🔍 [Adaptive Pipeline] Context Builder ({}) iniciando investigação...",
                pipeline_cfg.context_model
            ),
        )
        .await;

        let cb_start = Instant::now();
        let (handoff_summary, cb_tokens, cb_tool_calls) = Self::run_context_builder(
            session.clone(),
            ctx.clone(),
            &user_prompt,
            &pipeline_cfg.context_model,
            cancellation_token.child_token(),
        )
        .await?;

        metrics.context_builder = Some(PhaseMetrics {
            name: "Context Builder".to_string(),
            model: pipeline_cfg.context_model.clone(),
            input_tokens: cb_tokens.0,
            output_tokens: cb_tokens.1,
            cached_input_tokens: 0,
            tool_calls: cb_tool_calls,
            duration_ms: cb_start.elapsed().as_millis() as u64,
        });

        if cancellation_token.is_cancelled() {
            return Ok(None);
        }

        // -------------------------------------------------------------
        // Phase 2: Compaction
        // -------------------------------------------------------------
        let cb_total = cb_tokens.0 + cb_tokens.1;
        let should_compact = pipeline_cfg.compact_before_architect
            || cb_total >= pipeline_cfg.context_threshold as i64;

        let compacted_summary = if should_compact {
            info!("Starting Phase 2: Compaction before Architect");
            emit_pipeline_status(
                &session,
                &ctx,
                "📦 [Adaptive Pipeline] Executando compactação de contexto...",
            )
            .await;

            let before_tokens = cb_total;
            // The compacted summary preserves all findings in concise format
            let compacted = Self::compact_handoff_summary(&handoff_summary);
            let estimated_after_tokens = ((compacted.len() / 4) as i64).max(1000);

            metrics.compact = Some(CompactMetrics {
                before_tokens,
                after_tokens: estimated_after_tokens,
            });

            compacted
        } else {
            handoff_summary
        };

        if cancellation_token.is_cancelled() {
            return Ok(None);
        }

        // -------------------------------------------------------------
        // Phase 3: Architect (Astra / Flagship Model)
        // -------------------------------------------------------------
        info!(
            model = %pipeline_cfg.architect_model,
            "Starting Phase 3: Architect"
        );
        emit_pipeline_status(
            &session,
            &ctx,
            &format!(
                "🏛️ [Adaptive Pipeline] Architect ({}) analisando e decidindo estratégia...",
                pipeline_cfg.architect_model
            ),
        )
        .await;

        let ar_start = Instant::now();
        let (architect_response, ar_tokens, ar_tool_calls) = Self::run_architect(
            session.clone(),
            ctx.clone(),
            &user_prompt,
            &compacted_summary,
            &pipeline_cfg.architect_model,
            cancellation_token.child_token(),
        )
        .await?;

        metrics.architect = Some(PhaseMetrics {
            name: "Architect".to_string(),
            model: pipeline_cfg.architect_model.clone(),
            input_tokens: ar_tokens.0,
            output_tokens: ar_tokens.1,
            cached_input_tokens: 0,
            tool_calls: ar_tool_calls,
            duration_ms: ar_start.elapsed().as_millis() as u64,
        });

        if cancellation_token.is_cancelled() {
            return Ok(None);
        }

        // -------------------------------------------------------------
        // Phase 4: Decision & Execution (Direct vs. Delegate)
        // -------------------------------------------------------------
        let routing_decision = routing::parse_routing_decision(&architect_response);

        let final_text = match routing_decision {
            RoutingDecision::Direct { raw_response } => {
                info!("Architect selected ROUTE: DIRECT");
                emit_pipeline_status(
                    &session,
                    &ctx,
                    "⚡ [Adaptive Pipeline] Rota direta selecionada: Astra finalizou a implementação.",
                )
                .await;
                raw_response
            }
            RoutingDecision::Delegate {
                plan_summary,
                tasks,
            } => {
                info!(
                    tasks_count = tasks.len(),
                    "Architect selected ROUTE: DELEGATE"
                );
                emit_pipeline_status(
                    &session,
                    &ctx,
                    &format!(
                        "👥 [Adaptive Pipeline] Rota delegada: distribuindo {} subtarefa(s) para workers...",
                        tasks.len()
                    ),
                )
                .await;

                let mut combined_results = Vec::new();
                combined_results.push(format!("### Plano do Arquiteto\n{plan_summary}\n"));

                for (idx, task) in tasks.iter().enumerate() {
                    if cancellation_token.is_cancelled() {
                        break;
                    }

                    let worker_model =
                        task.complexity.resolve_model(&pipeline_cfg.worker_model);
                    emit_pipeline_status(
                        &session,
                        &ctx,
                        &format!(
                            "🛠️ [Worker {}/{}] Executando `{}` ({worker_model})...",
                            idx + 1,
                            tasks.len(),
                            task.name
                        ),
                    )
                    .await;

                    let wk_start = Instant::now();
                    let (wk_output, wk_tokens, wk_tools) = Self::run_worker(
                        session.clone(),
                        ctx.clone(),
                        task,
                        &worker_model,
                        cancellation_token.child_token(),
                    )
                    .await?;

                    metrics.workers.push(PhaseMetrics {
                        name: task.name.clone(),
                        model: worker_model,
                        input_tokens: wk_tokens.0,
                        output_tokens: wk_tokens.1,
                        cached_input_tokens: 0,
                        tool_calls: wk_tools,
                        duration_ms: wk_start.elapsed().as_millis() as u64,
                    });

                    combined_results.push(format!(
                        "#### Subtarefa: {}\n{}\n",
                        task.name, wk_output
                    ));
                }

                combined_results.join("\n")
            }
        };

        // -------------------------------------------------------------
        // Phase 5: Stats & Final Reporting
        // -------------------------------------------------------------
        metrics.total_wall_time_ms = pipeline_start.elapsed().as_millis() as u64;
        let stats_display = metrics.format_pipeline_stats();

        // Emit stats block
        emit_pipeline_status(&session, &ctx, &format!("\n```\n{stats_display}\n```")).await;

        let response_with_stats = format!("{final_text}\n\n---\n```\n{stats_display}\n```");
        Ok(Some(response_with_stats))
    }

    async fn run_context_builder(
        session: Arc<Session>,
        ctx: Arc<TurnContext>,
        user_prompt: &str,
        context_model: &str,
        cancel_token: CancellationToken,
    ) -> CodexResult<(String, (i64, i64), usize)> {
        let mut config = ctx.config.as_ref().clone();
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

        let (_delegate_session, io) = run_codex_thread_one_shot(
            config,
            Arc::clone(&session.services.auth_manager),
            Arc::clone(&session.services.models_manager),
            input,
            Arc::clone(&session),
            ctx.clone(),
            cancel_token,
            SubAgentSource::ThreadSpawn {
                parent_turn_id: ctx.sub_id.clone(),
                depth: 1,
            },
            /*final_output_json_schema*/ None,
            /*initial_history*/ None,
        )
        .await?;

        drain_events_and_collect(session, ctx, io.rx_event).await
    }

    async fn run_architect(
        session: Arc<Session>,
        ctx: Arc<TurnContext>,
        user_prompt: &str,
        handoff_summary: &str,
        architect_model: &str,
        cancel_token: CancellationToken,
    ) -> CodexResult<(String, (i64, i64), usize)> {
        let mut config = ctx.config.as_ref().clone();
        config.model = Some(architect_model.to_string());
        config.base_instructions = Some(prompts::ARCHITECT_SYSTEM_PROMPT.to_string());
        config.permissions.approval_policy = Constrained::allow_only(AskForApproval::Never);
        let _ = config.features.disable(Feature::Collab);
        let _ = config.features.disable(Feature::MultiAgentV2);
        let _ = config.features.disable(Feature::AdaptivePipeline);

        let input = vec![UserInput::Text {
            text: prompts::architect_input_prompt(user_prompt, handoff_summary),
            text_elements: Vec::new(),
        }];

        let (_delegate_session, io) = run_codex_thread_one_shot(
            config,
            Arc::clone(&session.services.auth_manager),
            Arc::clone(&session.services.models_manager),
            input,
            Arc::clone(&session),
            ctx.clone(),
            cancel_token,
            SubAgentSource::ThreadSpawn {
                parent_turn_id: ctx.sub_id.clone(),
                depth: 1,
            },
            /*final_output_json_schema*/ None,
            /*initial_history*/ None,
        )
        .await?;

        drain_events_and_collect(session, ctx, io.rx_event).await
    }

    async fn run_worker(
        session: Arc<Session>,
        ctx: Arc<TurnContext>,
        subtask: &routing::Subtask,
        worker_model: &str,
        cancel_token: CancellationToken,
    ) -> CodexResult<(String, (i64, i64), usize)> {
        let mut config = ctx.config.as_ref().clone();
        config.model = Some(worker_model.to_string());
        config.permissions.approval_policy = Constrained::allow_only(AskForApproval::Never);
        let _ = config.features.disable(Feature::Collab);
        let _ = config.features.disable(Feature::MultiAgentV2);
        let _ = config.features.disable(Feature::AdaptivePipeline);

        let input = vec![UserInput::Text {
            text: prompts::worker_task_prompt(
                &subtask.relevant_context,
                &subtask.task,
                &subtask.constraints,
                &subtask.acceptance_tests,
            ),
            text_elements: Vec::new(),
        }];

        // fork_turns="none": initial_history is None
        let (_delegate_session, io) = run_codex_thread_one_shot(
            config,
            Arc::clone(&session.services.auth_manager),
            Arc::clone(&session.services.models_manager),
            input,
            Arc::clone(&session),
            ctx.clone(),
            cancel_token,
            SubAgentSource::ThreadSpawn {
                parent_turn_id: ctx.sub_id.clone(),
                depth: 1,
            },
            /*final_output_json_schema*/ None,
            /*initial_history*/ None,
        )
        .await?;

        drain_events_and_collect(session, ctx, io.rx_event).await
    }

    fn compact_handoff_summary(handoff: &str) -> String {
        if let Some(idx) = handoff.find("# HANDOFF_SUMMARY") {
            handoff[idx..].trim().to_string()
        } else {
            handoff.trim().to_string()
        }
    }
}

async fn drain_events_and_collect(
    session: Arc<Session>,
    ctx: Arc<TurnContext>,
    receiver: async_channel::Receiver<Event>,
) -> CodexResult<(String, (i64, i64), usize)> {
    let mut collected_text = String::new();
    let mut input_tokens = 0i64;
    let mut output_tokens = 0i64;
    let mut tool_calls = 0usize;

    while let Ok(event) = receiver.recv().await {
        match event.msg {
            EventMsg::AgentMessage(AgentMessageEvent { message }) => {
                collected_text = message;
            }
            EventMsg::AgentMessageContentDelta(AgentMessageContentDeltaEvent { delta }) => {
                collected_text.push_str(&delta);
            }
            EventMsg::ItemCompleted(ItemCompletedEvent { item, .. }) => {
                if let codex_protocol::items::TurnItem::ToolCall(_) = item {
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
    }

    // Default fallback estimates if usage record was omitted
    if input_tokens == 0 && output_tokens == 0 && !collected_text.is_empty() {
        output_tokens = (collected_text.len() / 4) as i64;
        input_tokens = output_tokens.saturating_mul(4);
    }

    Ok((collected_text, (input_tokens, output_tokens), tool_calls))
}

async fn emit_pipeline_status(session: &Arc<Session>, ctx: &Arc<TurnContext>, message: &str) {
    session
        .send_event(
            ctx.as_ref(),
            EventMsg::AgentMessageContentDelta(AgentMessageContentDeltaEvent {
                delta: format!("\n{message}\n"),
            }),
        )
        .await;
}
