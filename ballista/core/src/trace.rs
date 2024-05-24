use opentelemetry::{global, propagation::Injector};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::runtime;
use std::ops::Deref;
use tonic::{
    metadata::{MetadataKey, MetadataMap, MetadataValue},
    Request, Status,
};
use tracing::{error, warn, Span, Subscriber};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

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
