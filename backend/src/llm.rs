//! The LLM/embedding seam. One hosted model (Gemini) serves the braindump
//! *cleaner* (ADR-0007), the ontology refactor's *pinned* generation
//! (ADR-0003), the chat *synthesizer* (ADR-0005), the *extractor*
//! (ADR-0001 / ADR-0002 / ADR-0010), and the *embedding client*
//! (ADR-0001 / ADR-0004). The seam is real — Gemini is a true-external
//! dependency — but the split across three near-identical traits was
//! over-decomposition (issue #39): `GeminiClient` implemented all three,
//! `AppState` held three `Arc`s cloning one client, and every test
//! substitution re-implemented the trio. They collapse to this single
//! [`Llm`] trait; tests swap in [`FakeLlm`] (or a scripted stand-in) so the
//! pipelines are hermetic. The real Gemini implementation lives in
//! [`crate::gemini`].

use async_trait::async_trait;

use crate::error::Result;
use crate::extractor::ExtractionResult;

/// The single LLM/embedding seam. One hosted model (Gemini) serves cleaning,
/// pinned generation, grounded synthesis, structured-output extraction, and
/// embeddings — so this is one trait, not three (issue #39). The real
/// implementation is [`crate::gemini::GeminiClient`]; tests swap in
/// [`FakeLlm`] or a scripted stand-in.
#[async_trait]
pub trait Llm: Send + Sync {
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

    /// Pull concepts and typed, directional edges out of a braindump's
    /// verbatim text (ADR-0001 / ADR-0002 / ADR-0010). Edge types are drawn
    /// from the governed ontology passed in `ontology_slugs`; the LLM never
    /// invents a type. Identity resolution, accretion, provenance, and
    /// type-history are NOT the extractor's job; they live in
    /// [`crate::graph`].
    async fn extract(
        &self,
        verbatim: &str,
        ontology_slugs: &[String],
    ) -> Result<ExtractionResult>;

    /// Embed text for storage (braindump / concept / type) — the *document*
    /// task type (ADR-0001 / ADR-0004).
    async fn embed_document(&self, text: &str) -> Result<Vec<f32>>;

    /// Embed a query used to seed retrieval — the *query* task type
    /// (ADR-0004).
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>>;

    /// Dimensionality of every emitted vector. Sized the `vec0` virtual tables
    /// at startup before any embed API call.
    fn dim(&self) -> usize;
}

/// A no-op LLM/embedding for tests: `clean` returns the verbatim (trimmed),
/// `generate_pinned` echoes the user prompt, `synthesize` returns a sentinel
/// marker so any test that forgets to script the LLM fails loudly, `extract`
/// returns an empty result, and `embed_*` returns a deterministic token-bucket
/// vector at `dim` (default 64). Never touches the network.
#[derive(Clone, Copy, Debug)]
pub struct FakeLlm {
    /// Dimensionality of the deterministic embedding. Defaults to 64.
    pub dim: usize,
}

impl Default for FakeLlm {
    fn default() -> Self {
        Self { dim: 64 }
    }
}

#[async_trait]
impl Llm for FakeLlm {
    async fn clean(&self, verbatim: &str) -> Result<String> {
        Ok(verbatim.trim().to_string())
    }
    async fn generate_pinned(&self, _system: &str, user: &str) -> Result<String> {
        Ok(user.to_string())
    }
    async fn synthesize(&self, _system: &str, _user: &str) -> Result<String> {
        Ok("FakeLlm::synthesize called — script an Llm in tests".to_string())
    }
    async fn extract(
        &self,
        _verbatim: &str,
        _ontology_slugs: &[String],
    ) -> Result<ExtractionResult> {
        Ok(ExtractionResult::default())
    }
    async fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
        Ok(crate::embedding::deterministic_vector(text, self.dim))
    }
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(crate::embedding::deterministic_vector(text, self.dim))
    }
    fn dim(&self) -> usize {
        self.dim
    }
}

#[cfg(test)]
mod characterization {
    use super::*;
    use crate::embedding::{cosine, deterministic_vector};

    /// The consolidated `FakeLlm` pins the behaviour the three former fakes
    /// (`FakeLlm` + `FakeExtractor` + `FakeEmbedding`) had before issue #39
    /// collapsed them. These characterization tests lock that behaviour at the
    /// single `Llm` seam so the refactor is provably behaviour-preserving.

    #[tokio::test]
    async fn clean_trims_verbatim() {
        let llm: &dyn Llm = &FakeLlm::default();
        assert_eq!(llm.clean("  hi  ").await.unwrap(), "hi");
    }

    #[tokio::test]
    async fn generate_pinned_echoes_the_user_prompt() {
        let llm: &dyn Llm = &FakeLlm::default();
        assert_eq!(llm.generate_pinned("sys", "usr").await.unwrap(), "usr");
    }

    #[tokio::test]
    async fn synthesize_returns_a_sentinel_so_unscripted_tests_fail_loudly() {
        let llm: &dyn Llm = &FakeLlm::default();
        let out = llm.synthesize("sys", "usr").await.unwrap();
        assert!(
            out.contains("script") || out.contains("FakeLlm"),
            "synthesize must not silently pass: {out}"
        );
    }

    #[tokio::test]
    async fn extract_returns_no_concepts_or_edges() {
        let llm: &dyn Llm = &FakeLlm::default();
        let result = llm.extract("the q3 review went off the rails", &[]).await.unwrap();
        assert!(result.concepts.is_empty(), "{result:?}");
        assert!(result.edges.is_empty(), "{result:?}");
    }

    #[tokio::test]
    async fn embed_document_matches_deterministic_vector_at_configured_dim() {
        let llm: &dyn Llm = &FakeLlm { dim: 32 };
        let v = llm.embed_document("q3 risk").await.unwrap();
        assert_eq!(v, deterministic_vector("q3 risk", 32));
    }

    #[tokio::test]
    async fn embed_query_matches_deterministic_vector_at_configured_dim() {
        let llm: &dyn Llm = &FakeLlm { dim: 32 };
        let v = llm.embed_query("q3").await.unwrap();
        assert_eq!(v, deterministic_vector("q3", 32));
    }

    #[tokio::test]
    async fn dim_defaults_to_64_and_respects_the_field() {
        assert_eq!(FakeLlm::default().dim(), 64);
        assert_eq!(FakeLlm { dim: 128 }.dim(), 128);
    }

    #[tokio::test]
    async fn identical_text_embeds_to_an_identical_vector() {
        let llm: &dyn Llm = &FakeLlm::default();
        let a = llm.embed_document("q3 is at risk").await.unwrap();
        let b = llm.embed_document("q3 is at risk").await.unwrap();
        assert_eq!(a, b);
        assert!((cosine(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn shared_token_yields_positive_similarity() {
        let llm: &dyn Llm = &FakeLlm { dim: 8 };
        let a = llm.embed_document("q3 risk from maria").await.unwrap();
        let b = llm.embed_document("q3 something else").await.unwrap();
        assert!(cosine(&a, &b) > 0.0);
    }
}
