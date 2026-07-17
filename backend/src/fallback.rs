//! Circuit-breaker fallback for the text-generation LLM seam (issue #86).
//!
//! On the free Gemini tier the primary text model (`GEMINI_TEXT_MODEL`, default
//! `gemini-2.0-flash`) regularly trips 429 quota-exceeded errors. This module
//! wraps a primary [`Llm`] with a fallback [`Llm`] behind a **circuit
//! breaker**: once the primary has failed `GEMINI_FALLBACK_MAX_ATTEMPTS`
//! (default 5) consecutive text-generation calls with transient errors
//! ([`Error::TransientLlm`] - 429 / 5xx / transport), subsequent
//! text-generation calls route to the fallback model
//! (`GEMINI_TEXT_MODEL_FALLBACK`, default `gemini-3.1-flash-lite`) until a
//! cooldown expires and a half-open probe of the primary succeeds.
//!
//! Only the **text-generation** methods (`clean`, `generate_pinned`,
//! `synthesize`, `extract`) pass through the circuit. Embeddings
//! (`embed_document`, `embed_query`) bypass it entirely and always hit the
//! primary - the embedding model is identity-calibrated (ADR-0001) and a
//! fallback embed model would break concept identity and mismatch the vec0
//! table dimensionality.
//!
//! ADR-0003 pins the ontology refactor (`generate_pinned`) to one model for
//! determinism. Routing a refactor through the fallback is an accepted, logged
//! **exception** to that guarantee - it exists only because the free tier
//! forces it, and it is observable via tracing so the degradation is never
//! silent. The ADR's free-tier-fallback addendum records the exception.
//!
//! Issue #99: the consecutive-failure counter is NOT reset by a primary
//! success while the circuit is closed - several text-gen paths (ingest, chat,
//! retrieval, refactor) share one breaker, and a success in one path must not
//! mask sustained failures in another. The counter only increments on a
//! transient failure and only resets when a half-open probe succeeds and closes
//! the circuit. Every counter transition is logged so the breaker state is
//! fully auditable. See the ADR-0003 issue #99 addendum for the root cause and
//! the rejected alternatives.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::error::Result;
use crate::extractor::ExtractionResult;
use crate::llm::Llm;

const DEFAULT_COOLDOWN_SECS: u64 = 3600;
const DEFAULT_MAX_ATTEMPTS: u32 = 5;

/// The circuit-breaker wrapper around a primary and fallback [`Llm`]. Text
/// generation is routed by the circuit state; embeddings always hit the
/// primary. Constructed from env in `main` ([`FallbackLlm::from_env`]) or with
/// an injectable clock for tests ([`FallbackLlm::new_for_test`]).
pub struct FallbackLlm {
    primary: Arc<dyn Llm>,
    fallback: Arc<dyn Llm>,
    max_attempts: u32,
    cooldown_ms: u64,
    state: Mutex<CircuitState>,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
}

#[derive(Default)]
struct CircuitState {
    consecutive_failures: u32,
    opened_at: Option<u64>,
}

enum Decision {
    Closed,
    Open,
    HalfOpen,
}

impl FallbackLlm {
    /// Build the wrapper from env, reading `GEMINI_FALLBACK_MAX_ATTEMPTS`
    /// (default 5) and `GEMINI_FALLBACK_COOLDOWN_SECS` (default 3600). Parse
    /// failures fall back to the defaults rather than failing startup.
    pub fn from_env(primary: Arc<dyn Llm>, fallback: Arc<dyn Llm>) -> Self {
        let max_attempts = std::env::var("GEMINI_FALLBACK_MAX_ATTEMPTS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MAX_ATTEMPTS);
        let cooldown_secs = std::env::var("GEMINI_FALLBACK_COOLDOWN_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_COOLDOWN_SECS);
        tracing::info!(
            max_attempts,
            cooldown_secs,
            "fallback circuit wired: after {max_attempts} consecutive transient \
             failures, text-generation routes to the fallback model for {cooldown_secs}s"
        );
        Self {
            primary,
            fallback,
            max_attempts,
            cooldown_ms: cooldown_secs.saturating_mul(1000),
            state: Mutex::new(CircuitState::default()),
            now: Box::new(real_clock_ms),
        }
    }

    /// Test constructor with an injectable clock so cooldown expiry is
    /// deterministic (advance the clock past `cooldown_ms` to force
    /// half-open). Production uses [`FallbackLlm::from_env`].
    pub fn new_for_test(
        primary: Arc<dyn Llm>,
        fallback: Arc<dyn Llm>,
        max_attempts: u32,
        cooldown_ms: u64,
        now: impl Fn() -> u64 + Send + Sync + 'static,
    ) -> Self {
        Self {
            primary,
            fallback,
            max_attempts,
            cooldown_ms,
            state: Mutex::new(CircuitState::default()),
            now: Box::new(now),
        }
    }

    fn decide(&self) -> Decision {
        let st = self.state.lock().expect("circuit mutex poisoned");
        match st.opened_at {
            None => Decision::Closed,
            Some(t) => {
                if (self.now)().saturating_sub(t) >= self.cooldown_ms {
                    Decision::HalfOpen
                } else {
                    Decision::Open
                }
            }
        }
    }

    /// AC6: log every fallback invocation with the reason so the degradation
    /// is observable. `debug` per call (avoids spamming on a sustained open
    /// circuit); the open/re-open transitions already log at `warn` so a
    /// default-level operator sees the switch, while the per-call `debug`
    /// gives the full audit trail #86 asks for.
    fn log_serving_fallback(&self, reason: &str) {
        tracing::debug!(reason, "fallback model serving a text-generation call");
    }

    fn record_success(&self, probe: bool) {
        let mut st = self.state.lock().expect("circuit mutex poisoned");
        if probe {
            tracing::info!(
                failures = st.consecutive_failures,
                "fallback circuit closed: half-open primary probe succeeded, \
                 consecutive-failure counter reset to 0 and the fallback model retired"
            );
            st.consecutive_failures = 0;
            st.opened_at = None;
        } else {
            tracing::debug!(
                consecutive_failures = st.consecutive_failures,
                "fallback circuit closed-path primary success: consecutive-failure \
                 counter left unchanged (a success in one text-gen path must not mask \
                 sustained failures in another - issue #99)"
            );
        }
    }

    fn record_failure_closed(&self) {
        let mut st = self.state.lock().expect("circuit mutex poisoned");
        st.consecutive_failures += 1;
        if st.consecutive_failures >= self.max_attempts && st.opened_at.is_none() {
            st.opened_at = Some((self.now)());
            tracing::warn!(
                failures = st.consecutive_failures,
                threshold = self.max_attempts,
                "fallback circuit opened: primary hit the consecutive-transient-failure \
                 threshold; subsequent text-generation calls route to the fallback model"
            );
        } else {
            tracing::debug!(
                consecutive_failures = st.consecutive_failures,
                threshold = self.max_attempts,
                "fallback circuit still closed: primary transient failure recorded"
            );
        }
    }

    fn record_failure_halfopen(&self) {
        let mut st = self.state.lock().expect("circuit mutex poisoned");
        st.opened_at = Some((self.now)());
        st.consecutive_failures = self.max_attempts;
        tracing::warn!(
            consecutive_failures = st.consecutive_failures,
            "fallback circuit re-opened: half-open primary probe failed; \
             falling back for this call and restarting the cooldown"
        );
    }
}

#[async_trait]
impl Llm for FallbackLlm {
    async fn clean(&self, verbatim: &str) -> Result<String> {
        match self.decide() {
            Decision::Open => {
                self.log_serving_fallback("circuit open");
                self.fallback.clean(verbatim).await
            }
            Decision::Closed => match self.primary.clean(verbatim).await {
                Ok(v) => {
                    self.record_success(false);
                    Ok(v)
                }
                Err(e) if e.is_transient() => {
                    self.record_failure_closed();
                    Err(e)
                }
                Err(e) => Err(e),
            },
            Decision::HalfOpen => match self.primary.clean(verbatim).await {
                Ok(v) => {
                    self.record_success(true);
                    Ok(v)
                }
                Err(e) if e.is_transient() => {
                    self.record_failure_halfopen();
                    self.fallback.clean(verbatim).await
                }
                Err(e) => Err(e),
            },
        }
    }

    async fn generate_pinned(&self, system: &str, user: &str) -> Result<String> {
        match self.decide() {
            Decision::Open => {
                self.log_serving_fallback("circuit open");
                self.fallback.generate_pinned(system, user).await
            }
            Decision::Closed => match self.primary.generate_pinned(system, user).await {
                Ok(v) => {
                    self.record_success(false);
                    Ok(v)
                }
                Err(e) if e.is_transient() => {
                    self.record_failure_closed();
                    Err(e)
                }
                Err(e) => Err(e),
            },
            Decision::HalfOpen => match self.primary.generate_pinned(system, user).await {
                Ok(v) => {
                    self.record_success(true);
                    Ok(v)
                }
                Err(e) if e.is_transient() => {
                    self.record_failure_halfopen();
                    self.fallback.generate_pinned(system, user).await
                }
                Err(e) => Err(e),
            },
        }
    }

    async fn synthesize(&self, system: &str, user: &str) -> Result<String> {
        match self.decide() {
            Decision::Open => {
                self.log_serving_fallback("circuit open");
                self.fallback.synthesize(system, user).await
            }
            Decision::Closed => match self.primary.synthesize(system, user).await {
                Ok(v) => {
                    self.record_success(false);
                    Ok(v)
                }
                Err(e) if e.is_transient() => {
                    // Issue #94: eager per-call failover for chat synthesis. On
                    // a transient primary failure with the circuit still
                    // closed, retry the same chat request on the fallback so
                    // the user never sees a raw 429/5xx. The failure still
                    // counts toward the consecutive-failure counter and may
                    // yet open the circuit; this is an addition for the chat
                    // path, not a replacement of the breaker. Non-transient
                    // errors fall through to the arm below and propagate as-is.
                    self.record_failure_closed();
                    self.log_serving_fallback("closed-circuit transient failover");
                    self.fallback.synthesize(system, user).await
                }
                Err(e) => Err(e),
            },
            Decision::HalfOpen => match self.primary.synthesize(system, user).await {
                Ok(v) => {
                    self.record_success(true);
                    Ok(v)
                }
                Err(e) if e.is_transient() => {
                    self.record_failure_halfopen();
                    self.fallback.synthesize(system, user).await
                }
                Err(e) => Err(e),
            },
        }
    }

    async fn extract(&self, verbatim: &str, ontology_slugs: &[String]) -> Result<ExtractionResult> {
        match self.decide() {
            Decision::Open => {
                self.log_serving_fallback("circuit open");
                self.fallback.extract(verbatim, ontology_slugs).await
            }
            Decision::Closed => match self.primary.extract(verbatim, ontology_slugs).await {
                Ok(v) => {
                    self.record_success(false);
                    Ok(v)
                }
                Err(e) if e.is_transient() => {
                    self.record_failure_closed();
                    Err(e)
                }
                Err(e) => Err(e),
            },
            Decision::HalfOpen => match self.primary.extract(verbatim, ontology_slugs).await {
                Ok(v) => {
                    self.record_success(true);
                    Ok(v)
                }
                Err(e) if e.is_transient() => {
                    self.record_failure_halfopen();
                    self.fallback.extract(verbatim, ontology_slugs).await
                }
                Err(e) => Err(e),
            },
        }
    }

    async fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
        self.primary.embed_document(text).await
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.primary.embed_query(text).await
    }

    fn dim(&self) -> usize {
        self.primary.dim()
    }
}

fn real_clock_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::extractor::ExtractedConcept;
    use crate::llm::Llm;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A scripted `Llm` for circuit-breaker tests. Text methods return a tagged
    /// marker on success (so the test can tell which model served) and fail
    /// with `TransientLlm` on the 1-based call indices listed in `fail_on`.
    /// Embeddings use the deterministic token-bucket vector at `dim_val` and
    /// are counted separately so the test can assert they bypass the circuit.
    struct ScriptLlm {
        tag: &'static str,
        fail_on: Vec<u32>,
        text_calls: AtomicU64,
        embed_calls: AtomicU64,
        dim_val: usize,
    }

    impl ScriptLlm {
        fn new(tag: &'static str, fail_on: Vec<u32>, dim_val: usize) -> Self {
            Self {
                tag,
                fail_on,
                text_calls: AtomicU64::new(0),
                embed_calls: AtomicU64::new(0),
                dim_val,
            }
        }
        fn text_call(&self) -> bool {
            let n = self.text_calls.fetch_add(1, Ordering::Relaxed) + 1;
            self.fail_on.contains(&(n as u32))
        }
        fn text_calls(&self) -> u64 {
            self.text_calls.load(Ordering::Relaxed)
        }
        fn embed_calls(&self) -> u64 {
            self.embed_calls.load(Ordering::Relaxed)
        }
        fn transient(&self) -> Error {
            Error::TransientLlm(format!("{}: simulated 429", self.tag))
        }
    }

    #[async_trait]
    impl Llm for ScriptLlm {
        async fn clean(&self, verbatim: &str) -> Result<String> {
            if self.text_call() {
                return Err(self.transient());
            }
            Ok(format!("{}:clean:{}", self.tag, verbatim))
        }
        async fn generate_pinned(&self, system: &str, user: &str) -> Result<String> {
            if self.text_call() {
                return Err(self.transient());
            }
            Ok(format!("{}:pinned:{}:{}", self.tag, system, user))
        }
        async fn synthesize(&self, system: &str, user: &str) -> Result<String> {
            if self.text_call() {
                return Err(self.transient());
            }
            Ok(format!("{}:synthesize:{}:{}", self.tag, system, user))
        }
        async fn extract(&self, _verbatim: &str, _ontology: &[String]) -> Result<ExtractionResult> {
            if self.text_call() {
                return Err(self.transient());
            }
            Ok(ExtractionResult {
                concepts: vec![ExtractedConcept {
                    label: format!("{}-concept", self.tag),
                }],
                edges: vec![],
            })
        }
        async fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
            self.embed_calls.fetch_add(1, Ordering::Relaxed);
            Ok(crate::embedding::deterministic_vector(text, self.dim_val))
        }
        async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
            self.embed_calls.fetch_add(1, Ordering::Relaxed);
            Ok(crate::embedding::deterministic_vector(text, self.dim_val))
        }
        fn dim(&self) -> usize {
            self.dim_val
        }
    }

    fn test_clock(start: u64) -> (Arc<AtomicU64>, impl Fn() -> u64 + Send + Sync + 'static) {
        let t = Arc::new(AtomicU64::new(start));
        let tc = t.clone();
        (t, move || tc.load(Ordering::Relaxed))
    }

    fn wrapper(
        primary: Arc<dyn Llm>,
        fallback: Arc<dyn Llm>,
        max_attempts: u32,
        cooldown_ms: u64,
        now: impl Fn() -> u64 + Send + Sync + 'static,
    ) -> FallbackLlm {
        FallbackLlm::new_for_test(primary, fallback, max_attempts, cooldown_ms, now)
    }

    #[tokio::test]
    async fn primary_success_serves_from_primary_without_touching_fallback() {
        let primary = Arc::new(ScriptLlm::new("P", vec![], 64));
        let fallback = Arc::new(ScriptLlm::new("F", vec![], 48));
        let (_clock, now) = test_clock(1000);
        let llm = wrapper(
            primary.clone() as Arc<dyn Llm>,
            fallback.clone() as Arc<dyn Llm>,
            5,
            60_000,
            now,
        );
        assert_eq!(llm.clean("hi").await.unwrap(), "P:clean:hi");
        assert_eq!(fallback.text_calls(), 0, "fallback never invoked");
        assert_eq!(primary.text_calls(), 1);
    }

    #[tokio::test]
    async fn five_consecutive_failures_open_circuit_then_fallback_serves() {
        let primary = Arc::new(ScriptLlm::new("P", vec![1, 2, 3, 4, 5], 64));
        let fallback = Arc::new(ScriptLlm::new("F", vec![], 48));
        let (_clock, now) = test_clock(1000);
        let llm = wrapper(
            primary.clone() as Arc<dyn Llm>,
            fallback.clone() as Arc<dyn Llm>,
            5,
            60_000,
            now,
        );
        for i in 1..=5u32 {
            let err = llm.clean(&format!("c{i}")).await.unwrap_err();
            assert!(err.is_transient(), "call {i} should be transient: {err}");
        }
        assert_eq!(primary.text_calls(), 5);
        assert_eq!(
            llm.clean("c6").await.unwrap(),
            "F:clean:c6",
            "circuit open -> fallback serves"
        );
        assert_eq!(primary.text_calls(), 5, "primary not called while open");
        assert_eq!(fallback.text_calls(), 1);
    }

    /// Issue #99: a successful text-gen call from one path (here `synthesize`,
    /// simulating chat) must NOT reset the consecutive-failure counter that
    /// failures from another path (here `clean`, simulating ingest) are
    /// accumulating. The circuit opens after 5 transient failures regardless of
    /// interspersed successes - the production incident root cause.
    #[tokio::test]
    async fn five_transient_failures_open_the_circuit_even_with_an_intervening_success() {
        let primary = Arc::new(ScriptLlm::new("P", vec![1, 3, 4, 5, 6], 64));
        let fallback = Arc::new(ScriptLlm::new("F", vec![], 48));
        let (_clock, now) = test_clock(1000);
        let llm = wrapper(
            primary.clone() as Arc<dyn Llm>,
            fallback.clone() as Arc<dyn Llm>,
            5,
            60_000,
            now,
        );
        // Ingest path fails (counter 1).
        assert!(llm.clean("ingest1").await.is_err());
        // Chat path SUCCEEDS against the primary - must not reset the counter.
        assert_eq!(
            llm.synthesize("sys", "usr").await.unwrap(),
            "P:synthesize:sys:usr",
            "the success came from the primary, not the fallback"
        );
        // Ingest path keeps failing (counter 2, 3, 4, 5 -> open on the 5th).
        assert!(llm.clean("ingest2").await.is_err());
        assert!(llm.clean("ingest3").await.is_err());
        assert!(llm.clean("ingest4").await.is_err());
        assert!(llm.clean("ingest5").await.is_err());
        // Circuit is now open: the next call routes to the fallback without
        // probing the primary, even though a chat success sat between the
        // ingest failures.
        assert_eq!(
            llm.clean("ingest6").await.unwrap(),
            "F:clean:ingest6",
            "circuit opened despite the intervening success -> fallback serves"
        );
        assert_eq!(
            primary.text_calls(),
            6,
            "primary attempted 6 times (5 transient failures + 1 success)"
        );
        assert_eq!(
            fallback.text_calls(),
            1,
            "fallback served exactly once, after the circuit opened"
        );
    }

    #[tokio::test]
    async fn cooldown_expiry_half_open_probe_success_closes_the_circuit() {
        let primary = Arc::new(ScriptLlm::new("P", vec![1, 2, 3, 4, 5], 64));
        let fallback = Arc::new(ScriptLlm::new("F", vec![], 48));
        let (clock, now) = test_clock(1000);
        let llm = wrapper(
            primary.clone() as Arc<dyn Llm>,
            fallback.clone() as Arc<dyn Llm>,
            5,
            60_000,
            now,
        );
        for _ in 0..5 {
            assert!(llm.clean("x").await.is_err());
        }
        assert_eq!(llm.clean("c6").await.unwrap(), "F:clean:c6");
        clock.fetch_add(60_001, Ordering::Relaxed);
        assert_eq!(
            llm.clean("c7").await.unwrap(),
            "P:clean:c7",
            "half-open probe used the primary"
        );
        assert_eq!(llm.clean("c8").await.unwrap(), "P:clean:c8");
        assert_eq!(fallback.text_calls(), 1, "fallback not used after close");
    }

    #[tokio::test]
    async fn half_open_probe_failure_reopens_circuit_and_falls_back_this_call() {
        let primary = Arc::new(ScriptLlm::new("P", vec![1, 2, 3, 4, 5, 6], 64));
        let fallback = Arc::new(ScriptLlm::new("F", vec![], 48));
        let (clock, now) = test_clock(1000);
        let llm = wrapper(
            primary.clone() as Arc<dyn Llm>,
            fallback.clone() as Arc<dyn Llm>,
            5,
            60_000,
            now,
        );
        for _ in 0..5 {
            assert!(llm.clean("x").await.is_err());
        }
        clock.fetch_add(60_001, Ordering::Relaxed);
        assert_eq!(
            llm.clean("probe").await.unwrap(),
            "F:clean:probe",
            "probe failed -> fell back for this call (no 503 to the user)"
        );
        assert_eq!(primary.text_calls(), 6, "primary was probed once");
        // Circuit re-opened -> next call uses fallback, not primary.
        assert_eq!(llm.clean("c8").await.unwrap(), "F:clean:c8");
        assert_eq!(
            primary.text_calls(),
            6,
            "primary not called while re-opened"
        );
    }

    #[tokio::test]
    async fn max_attempts_threshold_is_respected_below_it_circuit_stays_closed() {
        let primary = Arc::new(ScriptLlm::new("P", vec![1, 2, 3, 4], 64));
        let fallback = Arc::new(ScriptLlm::new("F", vec![], 48));
        let (_clock, now) = test_clock(1000);
        let llm = wrapper(
            primary.clone() as Arc<dyn Llm>,
            fallback.clone() as Arc<dyn Llm>,
            5,
            60_000,
            now,
        );
        for _ in 0..4 {
            assert!(llm.clean("x").await.is_err());
        }
        assert_eq!(
            fallback.text_calls(),
            0,
            "only 4 failures (< threshold 5) -> circuit closed, fallback unused"
        );
    }

    #[tokio::test]
    async fn non_transient_error_propagates_without_tripping_the_circuit() {
        let primary = Arc::new(NonTransientLlm);
        let fallback = Arc::new(ScriptLlm::new("F", vec![], 48));
        let (_clock, now) = test_clock(1000);
        let llm = wrapper(
            primary.clone() as Arc<dyn Llm>,
            fallback.clone() as Arc<dyn Llm>,
            5,
            60_000,
            now,
        );
        let err = llm.clean("x").await.unwrap_err();
        assert!(!err.is_transient(), "Internal error must propagate as-is");
        assert_eq!(
            fallback.text_calls(),
            0,
            "non-transient error must not trip the circuit or touch fallback"
        );
    }

    /// Always returns a non-transient `Internal` error on text calls - a
    /// malformed-response / auth / bad-request class that the circuit must NOT
    /// treat as a quota trip.
    struct NonTransientLlm;

    #[async_trait]
    impl Llm for NonTransientLlm {
        async fn clean(&self, _verbatim: &str) -> Result<String> {
            Err(Error::Internal("simulated bad request".into()))
        }
        async fn generate_pinned(&self, _: &str, _: &str) -> Result<String> {
            Err(Error::Internal("simulated bad request".into()))
        }
        async fn synthesize(&self, _: &str, _: &str) -> Result<String> {
            Err(Error::Internal("simulated bad request".into()))
        }
        async fn extract(&self, _: &str, _: &[String]) -> Result<ExtractionResult> {
            Err(Error::Internal("simulated bad request".into()))
        }
        async fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
            Ok(crate::embedding::deterministic_vector(text, 64))
        }
        async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
            Ok(crate::embedding::deterministic_vector(text, 64))
        }
        fn dim(&self) -> usize {
            64
        }
    }

    #[tokio::test]
    async fn embeddings_bypass_the_circuit_and_always_use_the_primary() {
        let primary = Arc::new(ScriptLlm::new("P", vec![1, 2, 3, 4, 5, 6, 7], 64));
        let fallback = Arc::new(ScriptLlm::new("F", vec![], 48));
        let (_clock, now) = test_clock(1000);
        let llm = wrapper(
            primary.clone() as Arc<dyn Llm>,
            fallback.clone() as Arc<dyn Llm>,
            5,
            60_000,
            now,
        );
        // Trip the circuit open with 5 transient text failures.
        for _ in 0..5 {
            assert!(llm.clean("x").await.is_err());
        }
        // Even with the circuit open, embed calls hit the primary only.
        let v = llm.embed_document("doc").await.unwrap();
        assert_eq!(v, crate::embedding::deterministic_vector("doc", 64));
        let q = llm.embed_query("qry").await.unwrap();
        assert_eq!(q, crate::embedding::deterministic_vector("qry", 64));
        assert_eq!(primary.embed_calls(), 2, "embeddings used the primary");
        assert_eq!(
            fallback.embed_calls(),
            0,
            "fallback never saw an embed call"
        );
        assert_eq!(
            llm.dim(),
            64,
            "dim comes from the primary, not the fallback"
        );
    }

    #[tokio::test]
    async fn fallback_serves_all_four_text_methods_when_circuit_is_open() {
        let primary = Arc::new(ScriptLlm::new("P", vec![1, 2, 3, 4, 5], 64));
        let fallback = Arc::new(ScriptLlm::new("F", vec![], 48));
        let (_clock, now) = test_clock(1000);
        let llm = wrapper(
            primary.clone() as Arc<dyn Llm>,
            fallback.clone() as Arc<dyn Llm>,
            5,
            60_000,
            now,
        );
        for _ in 0..5 {
            assert!(llm.clean("x").await.is_err());
        }
        // Circuit open -> every text method routes to the fallback.
        assert!(llm.clean("a").await.unwrap().starts_with("F:clean:"));
        assert!(llm
            .generate_pinned("s", "u")
            .await
            .unwrap()
            .starts_with("F:pinned:"));
        assert!(llm
            .synthesize("s", "u")
            .await
            .unwrap()
            .starts_with("F:synthesize:"));
        let extr = llm.extract("v", &[]).await.unwrap();
        assert_eq!(extr.concepts[0].label, "F-concept");
        assert_eq!(fallback.text_calls(), 4);
        assert_eq!(primary.text_calls(), 5, "primary untouched while open");
    }

    // --- issue #94: chat eager-failover on a transient primary synthesis ---

    /// Issue #94 AC1: on a transient primary `synthesize` failure with the
    /// circuit still closed, chat retries the same request on the fallback
    /// and returns the fallback's answer - the user never sees a raw 429/5xx.
    #[tokio::test]
    async fn synthesize_transient_primary_failure_failovers_to_fallback_on_closed_circuit() {
        let primary = Arc::new(ScriptLlm::new("P", vec![1], 64));
        let fallback = Arc::new(ScriptLlm::new("F", vec![], 48));
        let (_clock, now) = test_clock(1000);
        let llm = wrapper(
            primary.clone() as Arc<dyn Llm>,
            fallback.clone() as Arc<dyn Llm>,
            5,
            60_000,
            now,
        );
        let out = llm.synthesize("sys", "usr").await;
        assert!(
            out.is_ok(),
            "closed-circuit transient should failover, not surface an error: {:?}",
            out
        );
        assert_eq!(
            out.unwrap(),
            "F:synthesize:sys:usr",
            "the fallback's answer (tagged F) is returned to the user"
        );
        assert_eq!(primary.text_calls(), 1, "primary was attempted once");
        assert_eq!(fallback.text_calls(), 1, "fallback served the failover");
    }

    /// Issue #94 AC2: a non-transient primary `synthesize` error (malformed
    /// response / auth / bad request) propagates as-is - the fallback is NOT
    /// called and the circuit counter is NOT advanced.
    #[tokio::test]
    async fn synthesize_non_transient_error_propagates_without_tripping_fallback() {
        let primary = Arc::new(NonTransientLlm);
        let fallback = Arc::new(ScriptLlm::new("F", vec![], 48));
        let (_clock, now) = test_clock(1000);
        let llm = wrapper(
            primary.clone() as Arc<dyn Llm>,
            fallback.clone() as Arc<dyn Llm>,
            5,
            60_000,
            now,
        );
        let err = llm.synthesize("sys", "usr").await.unwrap_err();
        assert!(
            !err.is_transient(),
            "non-transient error must propagate as-is: {err}"
        );
        assert_eq!(
            fallback.text_calls(),
            0,
            "non-transient error must not trigger the fallback"
        );
    }

    /// Issue #94 AC3: a transient primary `synthesize` failure still feeds the
    /// circuit-breaker's consecutive-failure counter. After `max_attempts` (5)
    /// transient synthesize failures - each eagerly failovered to the fallback
    /// so the user sees an answer - the circuit opens; the next synthesize call
    /// goes directly to the fallback without probing the primary.
    #[tokio::test]
    async fn synthesize_transient_failover_still_advances_the_circuit_counter() {
        let primary = Arc::new(ScriptLlm::new("P", vec![1, 2, 3, 4, 5], 64));
        let fallback = Arc::new(ScriptLlm::new("F", vec![], 48));
        let (_clock, now) = test_clock(1000);
        let llm = wrapper(
            primary.clone() as Arc<dyn Llm>,
            fallback.clone() as Arc<dyn Llm>,
            5,
            60_000,
            now,
        );
        // 5 transient primary failures on `synthesize` - each eagerly failovers
        // to the fallback so the user sees an answer, not a 429/5xx.
        for i in 1..=5u32 {
            let out = llm.synthesize("s", "u").await;
            assert!(
                out.is_ok(),
                "call {i} should failover to the fallback, not error: {out:?}"
            );
            assert_eq!(
                out.unwrap(),
                "F:synthesize:s:u",
                "call {i} served by the fallback"
            );
        }
        assert_eq!(
            primary.text_calls(),
            5,
            "primary attempted once per call (5 transient failures)"
        );
        assert_eq!(fallback.text_calls(), 5, "fallback served every failover");
        // The 5 transient failures advanced the counter to the threshold and
        // opened the circuit. The next synthesize goes straight to the fallback
        // without touching the primary.
        assert_eq!(
            llm.synthesize("s", "u").await.unwrap(),
            "F:synthesize:s:u",
            "circuit open -> fallback serves"
        );
        assert_eq!(
            primary.text_calls(),
            5,
            "circuit open: primary not probed on the 6th call"
        );
        assert_eq!(fallback.text_calls(), 6);
    }
}
