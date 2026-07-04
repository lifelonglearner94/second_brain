//! In-memory bounded ring buffer of recent structured log events, plus a
//! `tracing_subscriber::Layer` that feeds it. The `/admin/logs` endpoint reads
//! this buffer so the hidden admin tab can surface backend errors (e.g. Gemini
//! generation failures) on the phone — the backend reading its own logs.
//!
//! Bounded by capacity: fixed memory, VPS-safe for the 8 GB single-user box.
//! Logs are operational, not epistemic state, so they live in RAM (never the
//! Brain File) and are lost on restart — the admin tab is a live tail, not an
//! audit log.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::Serialize;

/// Default retention. ~1000 structured entries is trivial RAM on an 8 GB box
/// yet covers a full session's worth of errors without paging.
pub const DEFAULT_CAPACITY: usize = 1_000;

/// One captured `tracing` event, serialised for the admin tab. `fields` holds
/// the event's structured context (everything but the message) as a JSON object.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct LogEntry {
    pub timestamp: i64,
    pub level: String,
    pub target: String,
    pub message: String,
    pub fields: serde_json::Value,
}

/// A bounded, thread-safe ring buffer of the most recent [`LogEntry`]s. Fixed
/// capacity → fixed memory; the oldest entry is evicted when the buffer is full.
#[derive(Clone)]
pub struct LogBuffer {
    inner: Arc<Mutex<VecDeque<LogEntry>>>,
    capacity: usize,
}

impl LogBuffer {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity,
        }
    }

    pub fn with_default_capacity() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Append an entry, evicting the oldest when at capacity.
    pub fn push(&self, entry: LogEntry) {
        let mut buf = self.inner.lock().expect("log buffer mutex poisoned");
        if buf.len() >= self.capacity {
            buf.pop_front();
        }
        buf.push_back(entry);
    }

    /// Up to `limit` most-recent entries, oldest-first (chronological order).
    pub fn recent(&self, limit: usize) -> Vec<LogEntry> {
        let buf = self.inner.lock().expect("log buffer mutex poisoned");
        let start = buf.len().saturating_sub(limit);
        buf.iter().skip(start).cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("log buffer mutex poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A `tracing_subscriber::Layer` that appends every observed event (that
/// survives the subscriber's filter) into a [`LogBuffer`]. Install it
/// alongside the fmt layer so the admin tab sees exactly what the operator's
/// `RUST_LOG` lets through — no more, no less.
pub struct LogBufferLayer {
    buffer: LogBuffer,
}

impl LogBufferLayer {
    pub fn new(buffer: LogBuffer) -> Self {
        Self { buffer }
    }
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl<S> tracing_subscriber::Layer<S> for LogBufferLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = LogEntryVisitor::new();
        event.record(&mut visitor);
        let entry = LogEntry {
            timestamp: now_seconds(),
            level: event.metadata().level().as_str().to_string(),
            target: event.metadata().target().to_string(),
            message: visitor.message,
            fields: serde_json::Value::Object(visitor.fields),
        };
        self.buffer.push(entry);
    }
}

/// `tracing::field::Visit` impl that drains an event's fields into a message
/// string + a JSON object of the remaining structured context.
struct LogEntryVisitor {
    message: String,
    fields: serde_json::Map<String, serde_json::Value>,
}

impl LogEntryVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
            fields: serde_json::Map::new(),
        }
    }

    fn store(&mut self, field: &tracing::field::Field, value: serde_json::Value) {
        if field.name() == "message" {
            self.message = match value {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
        } else {
            self.fields.insert(field.name().to_string(), value);
        }
    }
}

impl tracing::field::Visit for LogEntryVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.store(field, serde_json::Value::String(value.to_string()));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.store(field, serde_json::json!(value));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.store(field, serde_json::json!(value));
    }

    fn record_u128(&mut self, field: &tracing::field::Field, value: u128) {
        self.store(field, serde_json::json!(value));
    }

    fn record_i128(&mut self, field: &tracing::field::Field, value: i128) {
        self.store(field, serde_json::json!(value));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.store(field, serde_json::json!(value));
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.store(field, serde_json::json!(value));
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.store(field, serde_json::Value::String(format!("{:?}", value)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    fn entry(message: &str) -> LogEntry {
        LogEntry {
            timestamp: 0,
            level: "INFO".to_string(),
            target: "test".to_string(),
            message: message.to_string(),
            fields: serde_json::Value::Null,
        }
    }

    #[test]
    fn push_bounded_by_capacity_evicting_oldest() {
        let buf = LogBuffer::with_capacity(3);
        for i in 0..5 {
            buf.push(entry(&format!("e{i}")));
        }
        // Only the last `capacity` entries survive; the oldest are evicted.
        let recent = buf.recent(usize::MAX);
        assert_eq!(recent.len(), 3, "bounded to capacity: {recent:?}");
        assert_eq!(recent[0].message, "e2", "oldest evicted first");
        assert_eq!(recent[2].message, "e4", "newest is last (chronological)");
    }

    #[test]
    fn recent_limit_returns_only_the_newest_n() {
        let buf = LogBuffer::with_capacity(10);
        for i in 0..4 {
            buf.push(entry(&format!("e{i}")));
        }
        let recent = buf.recent(2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].message, "e2");
        assert_eq!(recent[1].message, "e3");
    }

    #[test]
    fn recent_on_empty_buffer_is_empty() {
        let buf = LogBuffer::with_default_capacity();
        assert!(buf.recent(10).is_empty());
        assert!(buf.is_empty());
    }

    #[test]
    fn log_buffer_layer_captures_emitted_event() {
        let buf = LogBuffer::with_default_capacity();
        let subscriber = tracing_subscriber::registry().with(LogBufferLayer::new(buf.clone()));
        let _guard = tracing::subscriber::set_default(subscriber);
        tracing::warn!(
            request_id = "abc123",
            retries = 3,
            "gemini generation failed"
        );

        let recent = buf.recent(usize::MAX);
        assert_eq!(recent.len(), 1, "exactly one event captured: {recent:?}");
        let e = &recent[0];
        assert_eq!(e.level, "WARN");
        assert!(!e.target.is_empty(), "target is the emitting module path");
        assert_eq!(e.message, "gemini generation failed");
        assert_eq!(e.fields["request_id"], "abc123");
        assert_eq!(e.fields["retries"], serde_json::json!(3));
    }
}
