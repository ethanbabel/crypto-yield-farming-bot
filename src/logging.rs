// Centralized logging setup for tracing with runtime log level, file logging, and structured output
use std::env;
use std::fs;
use tracing_subscriber::{
    fmt,
    EnvFilter, 
    layer::{SubscriberExt, Layer, Context}, 
    util::SubscriberInitExt
};
use tracing::{Id, Subscriber, span, field::Field, field::Visit, debug};
use std::time::{Instant, Duration};
use std::sync::OnceLock; // For global file guard

static FILE_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

pub fn init_logging() {
    // Load log levels for console and file from env
    let console_log_level = env::var("CONSOLE_LOG_LEVEL").unwrap_or_else(|_| "INFO".to_string());
    let file_log_level = env::var("FILE_LOG_LEVEL").unwrap_or_else(|_| "INFO".to_string());

    // Load file log flag from env
    let log_to_file = env::var("LOG_TO_FILE").unwrap_or_else(|_| "false".to_string()) == "true";

    // Set up EnvFilter for runtime log levels, filter globally to "warn", filter our own crate to the specified levels in .env
    let env_filter_console = EnvFilter::try_new(
        &format!("warn, crypto_yield_farming_bot={}", console_log_level)
    ).unwrap_or_else(|_| EnvFilter::new("crypto_yield_farming_bot=info"));

    let env_filter_file = EnvFilter::try_new(
        &format!("warn, crypto_yield_farming_bot={}", file_log_level)
    ).unwrap_or_else(|_| EnvFilter::new("crypto_yield_farming_bot=info"));

    // Console layer: always enabled, pretty human-readable logs
    let console_layer = fmt::Layer::new()
        .pretty()
        .with_filter(env_filter_console);

    let timing_layer = SpanTimingLayer;

    if log_to_file {   
        // Generate log file path with timestamp
        let timestamp = chrono::Local::now().format("%Y-%m-%d_%H:%M:%S").to_string();
        let log_dir = std::path::Path::new("logs");
        fs::create_dir_all(log_dir).expect("Failed to create log directory");
        let log_file_name = format!("{}.log", timestamp);
        let log_file_path = log_dir.join(log_file_name);

        let file_appender = tracing_appender::rolling::never(log_dir, log_file_path.file_name().unwrap());
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        FILE_GUARD.set(guard).ok(); // Store the guard globally

        // File layer: structured JSON logs with UTC timestamps
        let file_layer = fmt::Layer::new()
            .json()
            .with_writer(non_blocking)
            .with_timer(fmt::time::UtcTime::rfc_3339())
            .with_filter(env_filter_file);

        tracing_subscriber::registry()
            .with(console_layer)
            .with(file_layer)
            .with(timing_layer)
            .init();
    } else {
        // If not logging to file, just use console layer with timing
        tracing_subscriber::registry()
            .with(console_layer)
            .with(timing_layer)
            .init();
    }
}

// Custom layer to track span timing for specific spans with "on_close" field = true
struct SpanTimingLayer;

struct StartInstant(Instant);
struct LastInstant(Instant);
struct BusyTime(Duration);
struct IdleTime(Duration);

impl<S> Layer<S> for SpanTimingLayer
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut on_close = false;
            struct OnCloseVisitor<'a> { on_close: &'a mut bool }
            impl<'a> Visit for OnCloseVisitor<'a> {
                fn record_bool(&mut self, field: &Field, value: bool) {
                    if field.name() == "on_close" {
                        *self.on_close = value;
                    }
                }
                fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
            }
            let mut visitor = OnCloseVisitor { on_close: &mut on_close };
            attrs.record(&mut visitor);
            if on_close {
                let mut extensions = span.extensions_mut();
                let now = Instant::now();
                extensions.insert(StartInstant(now)); // start time
                extensions.insert(LastInstant(now)); // last enter/exit time
                extensions.insert(BusyTime(Duration::ZERO)); // busy time
                extensions.insert(IdleTime(Duration::ZERO)); // idle time
            }
        }
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut exts = span.extensions_mut();
            let last_instant = exts.remove::<LastInstant>().map(|li| li.0).unwrap_or_else(Instant::now);
            let mut busy_time = exts.remove::<BusyTime>().map(|bt| bt.0).unwrap_or(Duration::ZERO);
            busy_time += last_instant.elapsed();
            exts.insert(BusyTime(busy_time));
            exts.insert(LastInstant(Instant::now()));
        }
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut exts = span.extensions_mut();
            let last_instant = exts.remove::<LastInstant>().map(|li| li.0).unwrap_or_else(Instant::now);
            let mut idle_time = exts.remove::<IdleTime>().map(|it| it.0).unwrap_or(Duration::ZERO);
            idle_time += last_instant.elapsed();
            exts.insert(IdleTime(idle_time));
            exts.insert(LastInstant(Instant::now()));
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id) {
            let mut exts = span.extensions_mut();
            if let Some(StartInstant(start)) = exts.remove::<StartInstant>() {
                let total_duration = start.elapsed();
                let busy_time = exts.remove::<BusyTime>().map(|bt| bt.0).unwrap_or(Duration::ZERO);
                let idle_time = exts.remove::<IdleTime>().map(|it| it.0).unwrap_or(Duration::ZERO);
                debug!(
                    span = span.name(),
                    busy_time = ?busy_time,
                    idle_time = ?idle_time,
                    total_time = ?total_duration,
                    "span closed"
                );
            }
        }
    }
}
