use std::env;

use opentelemetry::{trace::TracerProvider as _, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    runtime,
    trace::{RandomIdGenerator, Sampler, TracerProvider},
    Resource,
};
use opentelemetry_semantic_conventions::{
    attribute::{DEPLOYMENT_ENVIRONMENT_NAME, SERVICE_NAME, SERVICE_VERSION},
    SCHEMA_URL,
};
use tracing_core::Level;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// Create a Resource that captures information about the entity for which telemetry is recorded.
fn resource() -> Resource {
    Resource::from_schema_url(
        [
            KeyValue::new(SERVICE_NAME, env!("CARGO_PKG_NAME")),
            KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
            KeyValue::new(DEPLOYMENT_ENVIRONMENT_NAME, "develop"),
        ],
        SCHEMA_URL,
    )
}

// Construct MeterProvider for MetricsLayer
// fn init_meter_provider() -> SdkMeterProvider {
//     let exporter = opentelemetry_otlp::MetricExporter::builder()
//         .with_tonic()
//         .with_temporality(opentelemetry_sdk::metrics::Temporality::default())
//         .build()
//         .unwrap();

//     let reader = PeriodicReader::builder(exporter, runtime::Tokio)
//         .with_interval(std::time::Duration::from_secs(30))
//         .build();

//     // For debugging in development
//     let stdout_reader = PeriodicReader::builder(
//         opentelemetry_stdout::MetricExporter::default(),
//         runtime::Tokio,
//     )
//     .build();

//     let meter_provider = MeterProviderBuilder::default()
//         .with_resource(resource())
//         .with_reader(reader)
//         .with_reader(stdout_reader)
//         .build();

//     global::set_meter_provider(meter_provider.clone());

//     meter_provider
// }

fn init_tracer_provider(address: &str) -> TracerProvider {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(address)
        .build()
        .unwrap();

    TracerProvider::builder()
        // Customize sampling strategy
        .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
            1.0,
        ))))
        // If export trace to AWS X-Ray, you can use XrayIdGenerator
        .with_id_generator(RandomIdGenerator::default())
        .with_resource(resource())
        .with_batch_exporter(exporter, runtime::Tokio)
        .build()
}

// Initialize tracing-subscriber and return OtelGuard for opentelemetry-related termination processing
pub(crate) fn init_tracing_subscriber() -> OtelGuard {
    let partial = tracing_subscriber::registry()
        // The global level filter prevents the exporter network stack
        // from reentering the globally installed OpenTelemetryLayer with
        // its own spans while exporting, as the libraries should not use
        // tracing levels below DEBUG. If the OpenTelemetry layer needs to
        // trace spans and events with higher verbosity levels, consider using
        // per-layer filtering to target the telemetry layer specifically,
        // e.g. by target matching.
        .with(tracing_subscriber::filter::LevelFilter::from_level(
            Level::INFO,
        ))
        .with(tracing_subscriber::fmt::layer());

    if let Ok(address) = env::var("OTEL_COLLECTOR_URL") {
        // address will be normlly: http://jaeger:4317
        let tracer_provider = init_tracer_provider(&address);
        // let meter_provider = init_meter_provider();
        let tracer = tracer_provider.tracer("tracing-otel-subscriber");
        partial
            .with(OpenTelemetryLayer::new(tracer))
            // .with(MetricsLayer::new(meter_provider.clone()))
            .init();
        OtelGuard {
            tracer_provider: Some(tracer_provider),
        }
    } else {
        partial.init();
        OtelGuard {
            tracer_provider: None,
        }
    }
}

pub(crate) struct OtelGuard {
    tracer_provider: Option<TracerProvider>,
    // meter_provider: SdkMeterProvider,
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        if let Some(tracer_provider) = &mut self.tracer_provider {
            if let Err(err) = tracer_provider.shutdown() {
                eprintln!("{err:?}");
            }
        }
        // if let Err(err) = self.meter_provider.shutdown() {
        //     eprintln!("{err:?}");
        // }
    }
}
