//! LLM client seam. The braindump *cleaner*, the *extractor*, and the chat
//! *synthesizer* all talk to a hosted model (Gemini) behind this trait; tests
//! swap in `FakeLlm` (or a scripted stand-in) so the pipelines are hermetic.
//! The real Gemini implementation lives in [`crate::gemini`].

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

    /// Grounded synthesis over retrieved context (ADR-0005). The caller
    /// (chat) builds a system prompt carrying the retrieved braindumps + edge
    /// paths and the citation/silence rules; this returns the model's
    /// synthesis. Temperature 0 so claims are reproducible, not free-handed.
    async fn synthesize(&self, system: &str, user: &str) -> Result<String>;
}

/// A no-op cleaner / generator for tests: returns the verbatim (trimmed) text
/// and echoes the prompt. `synthesize` returns a sentinel marker so any test
/// that forgets to script the LLM fails loudly rather than silently passing.
/// Never touches the network.
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
    async fn synthesize(&self, _system: &str, _user: &str) -> Result<String> {
        Ok("FakeLlm::synthesize called — script an LlmClient in tests".to_string())
    }
}
