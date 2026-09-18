use std::sync::Arc;

use super::SessionTask;
use super::SessionTaskResult;
use crate::session::TurnInput;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::state::TaskKind;
use codex_protocol::error::CodexErrorDetails;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Default)]
pub(crate) struct CompactTask;

impl SessionTask for CompactTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Compact
    }

    fn span_name(&self) -> &'static str {
        "session_task.compact"
    }

    async fn run(
        self: Arc<Self>,
        session: Arc<Session>,
        ctx: Arc<TurnContext>,
        _input: Vec<TurnInput>,
        _cancellation_token: CancellationToken,
    ) -> SessionTaskResult {
        let _profile_guard = ctx.turn_timing_state.begin_compaction();
        let result = crate::compact::compact_session(
            session,
            ctx,
            codex_analytics::CompactionTrigger::Manual,
        )
        .await;

        if let Err(err) = result {
            if matches!(err.details(), CodexErrorDetails::TurnAborted) {
                return Err(err);
            }
        }
        Ok(None)
    }
}
