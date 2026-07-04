//! LLM client seam. The braindump *cleaner* and the *extractor* both talk to a
//! hosted model (Gemini) behind this trait; tests swap in `FakeLlm` so the
//! ingest pipeline is hermetic. The real Gemini implementation lands in the
//! extraction slice.

use async_trait::async_trait;

use crate::error::Result;

#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Produce the cleaned, readable rendering of a braindump's verbatim text
    /// (ADR-0007). Shown by default in the UI.
    async fn clean(&self, verbatim: &str) -> Result<String>;

    /// Temperature-0 generation against a *pinned* model snapshot — used by the
    /// ontology refactor job (ADR-0003) so retagging stays deterministic across
    /// API model bumps. Returns the raw text response.
    async fn generate_pinned(&self, system: &str, user: &str) -> Result<String>;
}

/// A no-op cleaner / generator for tests: returns the verbatim (trimmed) text
/// and echoes the prompt. Never touches the network.
#[derive(Clone, Copy, Debug, Default)]
pub struct FakeLlm;

#[async_trait]
impl LlmClient for FakeLlm {
    async fn clean(&self, verbatim: &str) -> Result<String> {
        Ok(verbatim.trim().to_string())
    }
    async fn generate_pinned(&self, _system: &str, user: &str) -> Result<String> {
        Ok(user.to_string())
    }
}
