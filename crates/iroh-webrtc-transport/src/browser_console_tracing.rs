use std::sync::Once;

use wasm_bindgen::JsValue;

static CONSOLE_TRACING: Once = Once::new();

/// Install a browser console tracing subscriber for demo diagnostics.
///
/// This writes tracing events directly to the JavaScript console. It does not
/// send progress events or participate in browser worker control flow.
pub fn install_browser_console_tracing() {
    CONSOLE_TRACING.call_once(|| {
        use tracing_subscriber::prelude::*;

        let subscriber = tracing_subscriber::registry().with(BrowserConsoleLayer);
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}

pub(crate) fn install_worker_console_tracing() {
    install_browser_console_tracing();
}

struct BrowserConsoleLayer;

impl<S> tracing_subscriber::Layer<S> for BrowserConsoleLayer
where
    S: tracing::Subscriber,
{
    fn enabled(
        &self,
        metadata: &tracing::Metadata<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        matches!(
            *metadata.level(),
            tracing::Level::ERROR
                | tracing::Level::WARN
                | tracing::Level::INFO
                | tracing::Level::DEBUG
        ) && (metadata.target().starts_with("iroh_webrtc_transport")
            || metadata
                .target()
                .starts_with("iroh_webrtc_rust_browser_ping_pong"))
    }

    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();
        let mut fields = ConsoleTraceVisitor::default();
        event.record(&mut fields);

        let message = fields.message.unwrap_or_else(|| metadata.name().to_owned());
        let field_suffix = if fields.fields.is_empty() {
            String::new()
        } else {
            format!(" {}", fields.fields.join(" "))
        };
        let line = format!(
            "[{}] t={:.3} target={} {}{}",
            metadata.level(),
            js_sys::Date::now(),
            metadata.target(),
            message,
            field_suffix,
        );
        let line = JsValue::from_str(&line);
        match *metadata.level() {
            tracing::Level::ERROR => web_sys::console::error_1(&line),
            tracing::Level::WARN => web_sys::console::warn_1(&line),
            tracing::Level::INFO => web_sys::console::info_1(&line),
            tracing::Level::DEBUG | tracing::Level::TRACE => web_sys::console::debug_1(&line),
        }
    }
}

#[derive(Default)]
struct ConsoleTraceVisitor {
    message: Option<String>,
    fields: Vec<String>,
}

impl ConsoleTraceVisitor {
    fn record(&mut self, name: &str, value: String) {
        if name == "message" {
            self.message = Some(value);
        } else {
            self.fields.push(format!("{name}={value}"));
        }
    }
}

impl tracing::field::Visit for ConsoleTraceVisitor {
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.record(field.name(), value.to_string());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.record(field.name(), value.to_string());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.record(field.name(), value.to_string());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.record(field.name(), value.to_owned());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.record(field.name(), format!("{value:?}"));
    }
}
