//! Core coordinator for the Adaptive Model Pipeline V2.

use std::sync::Arc;
use std::time::Instant;

use codex_features::Feature;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::items::TurnItem;
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

use super::architect_selector::ArchitectSelection;
use super::architect_selector::ArchitectSelector;
use super::architect_selector::ArchitectTier;
use super::compaction::ArchitectContext;
use super::compaction::PipelineCompactionManager;
use super::metrics::ArchitectSelectionMetrics;
use super::metrics::PhaseMetrics;
use super::metrics::PipelineMetrics;
use super::prompts;
use super::prompts::EscalationAttempt;
use super::routing;
use super::routing::ArchitectStatus;
use super::routing::RoutingDecision;
use crate::codex_delegate::run_codex_thread_one_shot;
use crate::config::Constrained;
use crate::session::TurnInput;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;

pub(crate) struct PipelineCoordinator;

impl PipelineCoordinator {
    /// Executes the full adaptive pipeline flow.
    #[tracing::instrument(name = "adaptive_pipeline.run", skip_all)]
    pub(crate) async fn run(
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
        // Phase 1: Context Builder / Scout (Luna / Cheap Model)
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

        let scout_session = PipelineCompactionManager::spawn_scout(
            session.clone(),
            ctx.clone(),
            &user_prompt,
            &pipeline_cfg.context_model,
            cancellation_token.child_token(),
        )
        .await?;

        metrics.context_builder = Some(scout_session.metrics.clone());
        let assessment = scout_session.assessment.clone();

        if cancellation_token.is_cancelled() {
            scout_session.shutdown().await;
            return Ok(None);
        }

        // -------------------------------------------------------------
        // Phase 2: Native Compaction & ArchitectContext Preparation
        // -------------------------------------------------------------
        emit_pipeline_status(
            &session,
            &ctx,
            "📦 [Adaptive Pipeline] Avaliando política de compactação...",
        )
        .await;

        let (architect_context, compact_metrics) =
            PipelineCompactionManager::prepare_architect_context(
                &pipeline_cfg,
                &scout_session,
                ctx.clone(),
            )
            .await?;

        if let Some(cm) = &compact_metrics {
            metrics.compact = Some(cm.clone());
            emit_pipeline_status(
                &session,
                &ctx,
                &format!(
                    "📦 [Adaptive Pipeline] Contexto compactado: {} → {} tokens ({:.1}% redução)",
                    cm.before_tokens,
                    cm.after_tokens,
                    cm.reduction_percent()
                ),
            )
            .await;
        }

        // Shutdown the Scout session now that handoff / history snapshot is secured
        scout_session.shutdown().await;

        if cancellation_token.is_cancelled() {
            return Ok(None);
        }

        // -------------------------------------------------------------
        // Phase 3: Architect Selection (Auto vs. Explicit Bypass)
        // -------------------------------------------------------------
        let selection = ArchitectSelector::select(&pipeline_cfg, &assessment);
        let initially_selected_model = selection.model().to_string();

        let mut selection_metrics = ArchitectSelectionMetrics {
            mode: match &selection {
                ArchitectSelection::Explicit(_) => "explicit".to_string(),
                ArchitectSelection::Auto { .. } => "auto".to_string(),
            },
            score: match &selection {
                ArchitectSelection::Auto { score, .. } => Some(*score),
                ArchitectSelection::Explicit(_) => None,
            },
            initially_selected: initially_selected_model.clone(),
            final_model: initially_selected_model.clone(),
            escalations: 0,
        };

        let mut current_architect_model = initially_selected_model;
        emit_pipeline_status(
            &session,
            &ctx,
            &format!(
                "🏛️ [Adaptive Pipeline] Architect selecionado: {} (modo: {})",
                current_architect_model, selection_metrics.mode
            ),
        )
        .await;

        // -------------------------------------------------------------
        // Phase 4: Architect Execution & Escalation Loop
        // -------------------------------------------------------------
        let max_escalations = 2usize;
        let mut previous_attempt: Option<EscalationAttempt> = None;
        let mut architect_tokens = (0i64, 0i64);
        let mut architect_cached = 0i64;
        let mut architect_tool_calls = 0usize;
        let mut final_routing_decision: Option<RoutingDecision> = None;
        let mut final_architect_response = String::new();

        let architect_phase_start = Instant::now();

        while selection_metrics.escalations <= max_escalations {
            if cancellation_token.is_cancelled() {
                return Ok(None);
            }

            let (raw_response, tokens, cached, tools) = Self::run_architect_turn(
                session.clone(),
                ctx.clone(),
                &user_prompt,
                &architect_context,
                &assessment,
                &current_architect_model,
                previous_attempt.as_ref(),
                cancellation_token.child_token(),
            )
            .await?;

            architect_tokens.0 = architect_tokens.0.saturating_add(tokens.0);
            architect_tokens.1 = architect_tokens.1.saturating_add(tokens.1);
            architect_cached = architect_cached.saturating_add(cached);
            architect_tool_calls = architect_tool_calls.saturating_add(tools);
            final_architect_response = raw_response.clone();

            let status = routing::parse_architect_response(&raw_response);

            match status {
                ArchitectStatus::Escalate { reason, findings }
                    if pipeline_cfg.architect_auto_escalation
                        && selection_metrics.escalations < max_escalations =>
                {
                    let current_tier = ArchitectTier::from_model_name(&current_architect_model)
                        .unwrap_or(ArchitectTier::Terra);

                    if current_tier < ArchitectTier::Astra {
                        let next_tier = current_tier.next_tier();
                        let max_tier =
                            ArchitectTier::from_model_name(&pipeline_cfg.architect_auto_max)
                                .unwrap_or(ArchitectTier::Astra);
                        let target_tier = next_tier.min(max_tier);

                        if target_tier > current_tier {
                            selection_metrics.escalations =
                                selection_metrics.escalations.saturating_add(1);
                            emit_pipeline_status(
                                &session,
                                &ctx,
                                &format!(
                                    "⚠️ [Adaptive Pipeline] Architect ({current_architect_model}) solicitou escalonamento: \"{reason}\". Escalanado para {}...",
                                    target_tier.model_name()
                                ),
                            )
                            .await;

                            previous_attempt = Some(EscalationAttempt {
                                from_model: current_architect_model.clone(),
                                reason,
                                findings,
                            });
                            current_architect_model = target_tier.model_name().to_string();
                            continue;
                        }
                    }

                    // Cannot escalate further (already Astra or at max tier); proceed with fallback
                    info!("Architect requested escalation but reached ceiling. Treating as direct.");
                    final_routing_decision = Some(RoutingDecision::Direct {
                        raw_response,
                    });
                    break;
                }
                ArchitectStatus::Ready(decision) => {
                    final_routing_decision = Some(decision);
                    break;
                }
                _ => {
                    final_routing_decision = Some(RoutingDecision::Direct {
                        raw_response,
                    });
                    break;
                }
            }
        }

        selection_metrics.final_model = current_architect_model.clone();
        metrics.architect_selection = Some(selection_metrics);
        metrics.architect = Some(PhaseMetrics {
            name: "Architect".to_string(),
            model: current_architect_model,
            input_tokens: architect_tokens.0,
            output_tokens: architect_tokens.1,
            cached_input_tokens: architect_cached,
            tool_calls: architect_tool_calls,
            duration_ms: architect_phase_start.elapsed().as_millis() as u64,
        });

        // -------------------------------------------------------------
        // Phase 5: Decision & Workers Execution
        // -------------------------------------------------------------
        let routing_decision = final_routing_decision.unwrap_or_else(|| {
            RoutingDecision::Direct {
                raw_response: final_architect_response,
            }
        });

        let final_text = match routing_decision {
            RoutingDecision::Direct { raw_response } => {
                info!("Architect selected ROUTE: DIRECT");
                emit_pipeline_status(
                    &session,
                    &ctx,
                    "⚡ [Adaptive Pipeline] Rota direta selecionada: Architect finalizou a implementação.",
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

                    let worker_model = task.complexity.resolve_model_from_config(&pipeline_cfg);
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
                    let (wk_output, wk_tokens, wk_cached, wk_tools) = Self::run_worker(
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
                        cached_input_tokens: wk_cached,
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
        // Phase 6: Stats & Final Reporting
        // -------------------------------------------------------------
        metrics.total_wall_time_ms = pipeline_start.elapsed().as_millis() as u64;
        let stats_display = metrics.format_pipeline_stats();

        // Emit stats block
        emit_pipeline_status(&session, &ctx, &format!("\n```\n{stats_display}\n```")).await;

        let response_with_stats = format!("{final_text}\n\n---\n```\n{stats_display}\n```");
        Ok(Some(response_with_stats))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_architect_turn(
        session: Arc<Session>,
        ctx: Arc<TurnContext>,
        user_prompt: &str,
        context: &ArchitectContext,
        assessment: &super::architect_selector::ScoutAssessment,
        architect_model: &str,
        previous_escalation: Option<&EscalationAttempt>,
        cancel_token: CancellationToken,
    ) -> CodexResult<(String, (i64, i64), i64, usize)> {
        let mut config = ctx.config.as_ref().clone();
        config.model = Some(architect_model.to_string());
        config.base_instructions = Some(prompts::ARCHITECT_SYSTEM_PROMPT.to_string());
        config.permissions.approval_policy = Constrained::allow_only(AskForApproval::Never);
        let _ = config.features.disable(Feature::Collab);
        let _ = config.features.disable(Feature::MultiAgentV2);
        let _ = config.features.disable(Feature::AdaptivePipeline);

        let input_text = prompts::architect_context_prompt(
            user_prompt,
            context,
            assessment,
            previous_escalation,
        );

        let input = vec![UserInput::Text {
            text: input_text,
            text_elements: Vec::new(),
        }];

        // If ArchitectContext is CompactedHistory, seed with the forked history!
        let initial_history = match context {
            ArchitectContext::CompactedHistory {
                initial_history, ..
            } => Some(initial_history.clone()),
            ArchitectContext::ExplicitHandoff { .. } => None,
        };

        let (_delegate_session, io) = run_codex_thread_one_shot(
            config,
            Arc::clone(&session.services.auth_manager),
            Arc::clone(&session.services.models_manager),
            input,
            Arc::clone(&session),
            ctx.clone(),
            cancel_token,
            SubAgentSource::ThreadSpawn {
                parent_thread_id: session.thread_id(),
                depth: 1,
                agent_path: None,
                agent_nickname: None,
                agent_role: None,
            },
            /*final_output_json_schema*/ None,
            initial_history,
        )
        .await?;

        drain_events_and_collect(&io.rx_event).await
    }

    async fn run_worker(
        session: Arc<Session>,
        ctx: Arc<TurnContext>,
        subtask: &routing::Subtask,
        worker_model: &str,
        cancel_token: CancellationToken,
    ) -> CodexResult<(String, (i64, i64), i64, usize)> {
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

        // fork_turns="none": workers run isolated without inherited history
        let (_delegate_session, io) = run_codex_thread_one_shot(
            config,
            Arc::clone(&session.services.auth_manager),
            Arc::clone(&session.services.models_manager),
            input,
            Arc::clone(&session),
            ctx.clone(),
            cancel_token,
            SubAgentSource::ThreadSpawn {
                parent_thread_id: session.thread_id(),
                depth: 1,
                agent_path: None,
                agent_nickname: None,
                agent_role: None,
            },
            /*final_output_json_schema*/ None,
            /*initial_history*/ None,
        )
        .await?;

        drain_events_and_collect(&io.rx_event).await
    }
}

async fn drain_events_and_collect(
    receiver: &async_channel::Receiver<Event>,
) -> CodexResult<(String, (i64, i64), i64, usize)> {
    let mut collected_text = String::new();
    let mut input_tokens = 0i64;
    let mut output_tokens = 0i64;
    let mut cached_tokens = 0i64;
    let mut tool_calls = 0usize;

    while let Ok(event) = receiver.recv().await {
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
                cached_tokens = cached_tokens.max(info.total_token_usage.cached_input_tokens);
            }
            _ => {}
        }
    }

    if input_tokens == 0 && output_tokens == 0 && !collected_text.is_empty() {
        output_tokens = (collected_text.len() / 4) as i64;
        input_tokens = output_tokens.saturating_mul(4);
    }

    Ok((collected_text, (input_tokens, output_tokens), cached_tokens, tool_calls))
}

async fn emit_pipeline_status(session: &Arc<Session>, ctx: &Arc<TurnContext>, message: &str) {
    session
        .send_event(
            ctx.as_ref(),
            EventMsg::AgentMessageContentDelta(AgentMessageContentDeltaEvent {
                delta: format!("\n{message}\n"),
                thread_id: session.thread_id().to_string(),
                turn_id: ctx.sub_id.clone(),
                item_id: String::new(),
            }),
        )
        .await;
}
