use opentelemetry::propagation::Extractor;
use opentelemetry::{global, propagation::Injector};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::runtime;
use std::borrow::Cow;
use std::collections::HashMap;
use std::time::SystemTime;
use chrono::{DateTime, Utc};
use log::{error, info};
use tonic::metadata::KeyRef;
use tonic::{
    metadata::{MetadataKey, MetadataMap, MetadataValue},
    Request
};
use tracing::{warn, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

use opentelemetry::trace::{Event, Link, SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState, Tracer, TraceError};
use opentelemetry::{Context, Key, KeyValue, Value};
use opentelemetry_otlp::{SpanExporter as OTLPSpanExporter};
use opentelemetry_sdk::export::trace::{ExportResult, SpanData, SpanExporter};
use trace::ctx::SpanContext as IOXSpanContext;
use trace::ctx::{SpanId as IOXSpanId, TraceId as IOXTraceId};
use trace::span::{MetaValue, Span as IOXSpan, SpanStatus as IOXSpanStatus};

pub fn build_exporter() -> Result<OTLPSpanExporter, TraceError> {
    opentelemetry_otlp::new_exporter().tonic().build_span_exporter()
}

pub fn build_tracer() -> Result<opentelemetry_sdk::trace::Tracer, TraceError> {
    let otlp_exporter = opentelemetry_otlp::new_exporter().tonic();
    let tmp = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(otlp_exporter)
        .install_batch(runtime::Tokio);
    tmp
}

pub fn init_tracing(fmt_layer: Box<dyn Layer<Registry> + Send + Sync + 'static>) {
    global::set_text_map_propagator(TraceContextPropagator::new());
    // global::set_error_handler(|error| error!(error = format!("{error:#}"), "otel error"))
    //     .context("set error handler")?;

    let filter_layer = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap();

    let otlp_exporter = opentelemetry_otlp::new_exporter().tonic();

    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(otlp_exporter)
        // .install_simple()
        .install_batch(runtime::Tokio)
        .unwrap();
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(otel_layer)
        .with(filter_layer)
        .init();
}

/// Trace context propagation: send the trace context by injecting it into the metadata of the given
/// request.
/// PREREQUISITE: this needs to have called global::set_text_map_propagator
pub fn send_trace<T>(mut request: Request<T>) -> Result<Request<T>, Status> {
    global::get_text_map_propagator(|propagator| {
        let context = Span::current().context();
        propagator.inject_context(&context, &mut MetadataInjector(request.metadata_mut()))
    });

    Ok(request)
}

struct MetadataInjector<'a>(&'a mut MetadataMap);

impl Injector for MetadataInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        match MetadataKey::from_bytes(key.as_bytes()) {
            Ok(key) => match MetadataValue::try_from(&value) {
                Ok(value) => {
                    self.0.insert(key, value);
                }

                Err(error) => {
                    warn!(value, error = format!("{error:#}"), "parse metadata value")
                }
            },

            Err(error) => warn!(key, error = format!("{error:#}"), "parse metadata key"),
        }
    }
}

pub struct MetadataExtractor<'a>(&'a MetadataMap);

impl<'a> Extractor for MetadataExtractor<'a> {
    /// Get a value for a key from the MetadataMap.  If the value can't be converted to &str, returns None
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|metadata| metadata.to_str().ok())
    }

    /// Collect all the keys from the MetadataMap.
    fn keys(&self) -> Vec<&str> {
        self.0
            .keys()
            .map(|key| match key {
                KeyRef::Ascii(v) => v.as_str(),
                KeyRef::Binary(v) => v.as_str(),
            })
            .collect::<Vec<_>>()
    }
}

fn convert_influx_status_to_otlp_status(input: IOXSpanStatus) -> Status {
    match input {
        IOXSpanStatus::Unknown => Status::Unset,
        IOXSpanStatus::Ok => Status::Ok,
        IOXSpanStatus::Err => Status::Error {
            description: Cow::default(),
        },
    }
}

fn convert_metavalue_to_value(input: &MetaValue) -> Value {
    match input {
        MetaValue::String(s) => Value::String(s.clone().into()),
        MetaValue::Float(f) => Value::F64(*f),
        MetaValue::Int(i) => Value::I64(*i),
        MetaValue::Bool(b) => Value::Bool(*b),
    }
}

fn convert_metadata_to_attributes(
    input: &HashMap<Cow<'static, str>, MetaValue>,
) -> Vec<KeyValue> {
    input
        .iter()
        .map(|kv| {
            let key = Key::new(String::from(kv.0.clone()));
            let value = convert_metavalue_to_value(&kv.1);
            KeyValue::new(key, value)
        })
        .collect::<Vec<KeyValue>>()
}

fn convert_influx_span_to_otlp_span_data(
    tracer: &opentelemetry_sdk::trace::Tracer,
    span: &IOXSpan,
) -> SpanData {
    let start_time: SystemTime = span
        .start
        .unwrap_or_else(|| chrono::offset::Utc::now())
        .into();
    let end_time: SystemTime = span
        .end
        .unwrap_or_else(|| chrono::offset::Utc::now())
        .into();

    let mut builder = tracer
        .span_builder(span.name.clone())
        .with_trace_id(TraceId::from_bytes(u128::to_be_bytes(
            span.ctx.trace_id.get(),
        )))
        .with_span_id(SpanId::from_bytes(u64::to_be_bytes(span.ctx.span_id.get())))
        .with_kind(SpanKind::Internal)
        .with_start_time(start_time)
        .with_end_time(end_time)
        .with_attributes(convert_metadata_to_attributes(&span.metadata));

    let out_links = span
        .ctx
        .links
        .iter()
        .map(|link| {
            let ti = TraceId::from_bytes(u128::to_be_bytes(link.0.get()));
            let si = SpanId::from_bytes(u64::to_be_bytes(link.1.get()));
            let span_context =
                SpanContext::new(ti, si, TraceFlags::SAMPLED, false, TraceState::NONE);
            Link::new(span_context, vec![])
        })
        .collect::<Vec<Link>>();
    builder = builder.with_links(out_links);
    let out_events: Vec<Event> = span
        .events
        .iter()
        .map(|span_event| {
            Event::new(
                span_event.msg.clone(),
                span_event.time.into(),
                convert_metadata_to_attributes(&span_event.metadata),
                0,
            )
        })
        .collect::<Vec<Event>>();
    builder = builder.with_events(out_events);

    let context = Context::new();

    let out_span: opentelemetry_sdk::trace::Span =
        tracer.build_with_context(builder, &context);
    let mut data = out_span.exported_data().unwrap();

    data.span_context = SpanContext::new(
        TraceId::from_bytes(u128::to_be_bytes(span.ctx.trace_id.get())),
        // TraceId::from_bytes(u128::to_be_bytes(span.ctx.trace_id.get())),
        SpanId::from_bytes(u64::to_be_bytes(span.ctx.span_id.get())),
        TraceFlags::default(),
        false,
        TraceState::default(),
    );

    if let Some(parent) = span.ctx.parent_span_id {
        data.parent_span_id = SpanId::from_bytes(u64::to_be_bytes(parent.get()))
    } else {
        data.parent_span_id = SpanId::INVALID;
    }
    data.span_kind = SpanKind::Internal;
    data.start_time = start_time;
    data.end_time = end_time;
    data.attributes = convert_metadata_to_attributes(&span.metadata);
    data.status = convert_influx_status_to_otlp_status(span.status);

    data
}

fn convert_influx_spans_to_otlp_span_data(
    tracer: &opentelemetry_sdk::trace::Tracer,
    input: Vec<IOXSpan>,
) -> Vec<SpanData> {
    input
        .iter()
        .map(|s| convert_influx_span_to_otlp_span_data(tracer, s))
        .collect::<Vec<SpanData>>()
}

pub fn enclosing_start_end_time(spans: &Vec<IOXSpan>) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let min_start_time = spans
        .iter()
        .filter_map(|s|s.start)
        .min();
    let max_end_time =spans
        .iter()
        .filter_map(|s|s.end)
        .max();
    (min_start_time, max_end_time)
}

pub async fn export_spans(
    input: Vec<IOXSpan>,
) -> ExportResult {
    match build_tracer() {
        Ok(tracer) => {
            match build_exporter() {
                Ok(mut span_exporter) => {
                    span_exporter
                        .export(convert_influx_spans_to_otlp_span_data(&tracer, input))
                        .await
                }
                Err(e) => {
                    error!("export_spans: Error creating exporter: {:?}", e);
                    Err("Error creating exporter".into())
                }
            }
        },
        Err(e) => {
            error!("export_spans: ERROR CREATING TRACER: {:?}", e);
            Err(format!("Error creating tracer: {:?}", e).into())
        }
    }
}

pub async fn export_spans_with_tracer(
    tracer: &opentelemetry_sdk::trace::Tracer,
    input: Vec<IOXSpan>,
) -> ExportResult {
    let span_data = convert_influx_spans_to_otlp_span_data(&tracer, input);
    match build_exporter() {
        Ok(mut span_exporter) => {
            span_exporter
                .export(span_data)
                .await
        }
        Err(e) => {
            error!("export_spans: Error creating exporter: {:?}", e);
            Err("Error creating exporter".into())
        }
    }
}

