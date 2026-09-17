//! Session task wrapper for the Adaptive Model Pipeline.

use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::pipeline::PipelineCoordinator;
use crate::session::TurnInput;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::state::TaskKind;

use super::SessionTask;
use super::SessionTaskResult;

#[derive(Default)]
pub(crate) struct PipelineTask;

impl PipelineTask {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl SessionTask for PipelineTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Pipeline
    }

    fn span_name(&self) -> &'static str {
        "session_task.pipeline"
    }

    async fn run(
        self: Arc<Self>,
        sess: Arc<Session>,
        ctx: Arc<TurnContext>,
        input: Vec<TurnInput>,
        cancellation_token: CancellationToken,
    ) -> SessionTaskResult {
        sess.emit_turn_started(&ctx).await;
        PipelineCoordinator::run(sess, ctx, input, cancellation_token).await
    }
}
