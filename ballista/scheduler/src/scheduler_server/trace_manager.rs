use crate::cluster::JobState;
use crate::cluster::JobStateEvent::JobUpdated;
use crate::state::execution_graph::ExecutionStage::{Failed, Successful};
use crate::state::execution_graph::{ExecutionGraph, ExecutionStage};
use crate::state::SchedulerState;
use ballista_core::error::BallistaError;
use ballista_core::serde::protobuf::job_status::Status;
use ballista_core::trace::{build_exporter, build_tracer, enclosing_start_end_time, export_spans_with_tracer};
use datafusion_proto::logical_plan::AsLogicalPlan;
use datafusion_proto::physical_plan::AsExecutionPlan;
use futures::StreamExt;
use log::{error, info, warn};
use opentelemetry_otlp::SpanExporter as OTLPSpanExporter;
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use trace::ctx::{SpanContext, SpanId, TraceId};
use trace::span::{Span, SpanStatus};

pub(crate) struct TraceManager<T: 'static + AsLogicalPlan, U: 'static + AsExecutionPlan> {
    state: Arc<SchedulerState<T, U>>,
    job_state: Arc<dyn JobState>,
    span_exporter: Arc<OTLPSpanExporter>,
    tracer: Arc<opentelemetry_sdk::trace::Tracer>,
    stopped: Arc<AtomicBool>,
}

impl<T: 'static + AsLogicalPlan, U: 'static + AsExecutionPlan> TraceManager<T, U> {
    pub fn new(state: Arc<SchedulerState<T, U>>, job_state: Arc<dyn JobState>) -> Self {
        Self {
            state,
            job_state,
            span_exporter: Arc::new(build_exporter().unwrap()),
            tracer: Arc::new(build_tracer().unwrap()),
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn start(&mut self) -> ballista_core::error::Result<()> {
        if self.stopped.load(Ordering::SeqCst) {
            return Err(BallistaError::General(String::from(
                "TraceManager has already been stopped",
            )));
        }
        self.run().await;
        Ok(())
    }

    pub fn stop(&self) {
        if !self.stopped.swap(true, Ordering::SeqCst) {
        } else {
            // Keep quiet to allow calling `stop` multiple times.
        }
    }

    async fn run(&self) {
        let mut event_stream = self.job_state.job_state_events().await.unwrap();
        let stopped = self.stopped.clone();
        let state = self.state.clone();
        let tracer = self.tracer.clone();
        let mut span_exporter = self.span_exporter.clone();

        tokio::spawn(async move {
            info!("Starting the TraceManager event loop");
            while !stopped.load(Ordering::SeqCst) {
                if let Some(event) = event_stream.next().await {
                    info!("TraceManager got event ! {:?}", event);
                    match event {
                        JobUpdated { ref job_id, status } => {
                            let should_send = if let Some(job_status) = status.status {
                                match job_status {
                                    Status::Failed(job) => true,
                                    Status::Successful(job) => true,
                                    _ => false,
                                }
                            } else {
                                false
                            };
                            info!(
                                "TraceManager job updated: {}, should_send: {}",
                                job_id, should_send
                            );
                            if should_send {
                                if let Ok(execution_graph_opt) = state
                                    .task_manager
                                    .get_job_execution_graph(job_id.as_str())
                                    .await
                                {
                                    if let Some(execution_graph) = execution_graph_opt {
                                        let spans = make_spans_for_execution_graph(job_id, execution_graph.as_ref());
                                        info!("TraceManager computed spans: {:#?}", &spans);
                                        let export_result = export_spans_with_tracer(
                                            tracer.as_ref(),
                                            &mut span_exporter,
                                            spans
                                        ).await;
                                        info!("TraceManager export_result: {export_result:?}");

                                    } else {
                                        error!("TraceManager - couldn't get execution graph for job {}", job_id);
                                    }
                                } else {
                                    error!("TraceManager - couldn't get execution graph result for job {}", job_id);
                                }
                            }
                        }
                        _ => {}
                    };
                } else {
                    info!("Event Channel closed, shutting down");
                    break;
                }
            }
            info!("The TraceManager event loop has been stopped");
        });
    }
}

fn make_spans_for_execution_graph(job_id: &String, execution_graph: &ExecutionGraph) -> Vec<Span> {
    let job_span_context = SpanContext::new_with_optional_collector(None);
    let mut job_span = Span {
        name: Cow::from(format!("Job {}", job_id)),
        ctx: job_span_context,
        start: None,
        end: None,
        status: SpanStatus::Unknown,
        metadata: Default::default(),
        events: vec![],
    };

    let stages = execution_graph.stages();
    info!("TraceManager computing spans for {} stages", stages.len());
    let mut all_spans:Vec<Span> = vec![];
    for (part, stage) in stages.iter() {
        let mut stage_spans = make_spans_for_stage(&mut job_span, *part, stage);
        all_spans.append(&mut stage_spans);
    }
    let (job_start, job_end) = enclosing_start_end_time(&all_spans);
    job_span.start = job_start;
    job_span.end = job_end;
    all_spans.insert(0, job_span);
    all_spans
}

fn make_spans_for_stage(
    job_span: &mut Span,
    part: usize,
    stage: &ExecutionStage,
) -> Vec<Span> {
    info!("TraceManager: make_spans_for_stage part={part} {stage:?}");
    let mut remote_spans = match stage {
        Successful(stage) => {
            let tmp = stage
                .task_infos
                .iter()
                .map(|info| {
                    let json_trace = &info.json_trace;
                    info!("  TraceManager info: {}", json_trace);
                    if let Ok(spans) =
                        serde_json::from_str::<Vec<Span>>(json_trace.as_str())
                    {
                        spans
                    } else {
                        vec![]
                    }
                })
                .flatten()
                .collect::<Vec<Span>>();
            tmp
        }
        Failed(stage) => {
            let tmp = stage
                .task_infos
                .iter()
                .map(|info_opt| match info_opt {
                    Some(info) => {
                        let json_trace = &info.json_trace;
                        info!("  TraceManager info: {}", info.json_trace);
                        if let Ok(spans) =
                            serde_json::from_str::<Vec<Span>>(json_trace.as_str())
                        {
                            spans
                        } else {
                            vec![]
                        }
                    }
                    None => {
                        info!("  TraceManager info: NONE");
                        vec![]
                    }
                })
                .flatten()
                .collect::<Vec<Span>>();
            tmp
        }
        _ => {
            warn!("TraceManager got unknown stage !");
            vec![]
        }
    };
    let mut stage_context = SpanContext::new_with_optional_collector(None);
    stage_context.trace_id = TraceId::new(job_span.ctx.trace_id.get()).unwrap();
    stage_context.parent_span_id = Some(job_span.ctx.span_id);
    let (stage_start, stage_end) = enclosing_start_end_time(&remote_spans);
    let stage_span = Span {
        name: Cow::from(format!("ExecutionStage {}", part)),
        ctx: stage_context,
        start: stage_start,
        end: stage_end,
        status: SpanStatus::Unknown,
        metadata: Default::default(),
        events: vec![],
    };
    let span_ids = remote_spans
        .iter()
        .map(|s| s.ctx.span_id)
        .collect::<Vec<SpanId>>();
    info!("TraceManager span_ids: {:?}", span_ids);
    let parent_span_ids = remote_spans
        .iter()
        .filter_map(|s| s.ctx.parent_span_id)
        .collect::<Vec<SpanId>>();
    info!("TraceManager parent_span_ids: {:?}", parent_span_ids);
    let missing_parent_span_ids = parent_span_ids
        .iter()
        .filter(|&pid| !span_ids.contains(&pid))
        .collect::<Vec<&SpanId>>();
    info!(
        "TraceManager missing_parent_span_ids: {:?}",
        missing_parent_span_ids
    );
    let missing_parent_span_id =
        SpanId::new(missing_parent_span_ids.get(0).unwrap().get()).unwrap();
    info!(
        "TraceManager missing_parent_span_id: {:?}",
        missing_parent_span_id
    );
    remote_spans.iter_mut().for_each(|span| {
        span.ctx.trace_id = TraceId::new(job_span.ctx.trace_id.get()).unwrap();
        if span.ctx.parent_span_id.is_some()
            && span.ctx.parent_span_id.unwrap() == missing_parent_span_id
        {
            span.ctx.parent_span_id = Some(stage_span.ctx.span_id);
        }
    });
    remote_spans.insert(0, stage_span);
    info!("TraceManager final spans: {:?}", remote_spans);
    remote_spans
}
