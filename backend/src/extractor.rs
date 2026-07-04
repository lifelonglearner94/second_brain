//! Extraction seam (issue #5 / ADR-0001 / ADR-0002 / ADR-0010).
//!
//! The extractor pulls concepts and edges out of a braindump's verbatim text.
//! In this slice it is stubbed — [`FakeExtractor`] returns no concepts and no
//! edges — so the ingest pipeline is hermetic and the real Gemini-backed
//! extraction swaps in later without rewiring call sites. The cleaner is a
//! separate seam on [`crate::llm::LlmClient`].

use async_trait::async_trait;

use crate::error::Result;

/// A concept the LLM extracted from a braindump. Placeholder shape — the real
/// extraction slice adds embedding, identity-resolution, and provenance
/// (ADR-0001 / ADR-0010). The stub never produces one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExtractedConcept {
    pub label: String,
}

/// A typed, directional edge between two extracted concepts. Placeholder
/// shape — the real slice adds provenance and type-history (ADR-0002 /
/// ADR-0003). The stub never produces one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExtractedEdge {
    pub from_label: String,
    pub to_label: String,
    pub type_slug: String,
}

/// What the extractor returned from one braindump. Empty vecs from the stub.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExtractionResult {
    pub concepts: Vec<ExtractedConcept>,
    pub edges: Vec<ExtractedEdge>,
}

/// The extraction seam. Real implementation calls Gemini at Temperature=0 and
/// resolves identities against existing concepts (ADR-0001); the stub returns
/// an empty result so ingest wiring is exercised without network or DB churn.
#[async_trait]
pub trait Extractor: Send + Sync {
    async fn extract(&self, verbatim: &str) -> Result<ExtractionResult>;
}

/// No-op extractor for tests and the ingest skeleton. Returns no concepts and
/// no edges — never touches the network.
#[derive(Clone, Copy, Debug, Default)]
pub struct FakeExtractor;

#[async_trait]
impl Extractor for FakeExtractor {
    async fn extract(&self, _verbatim: &str) -> Result<ExtractionResult> {
        Ok(ExtractionResult::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_extractor_returns_no_concepts_or_edges() {
        let result = FakeExtractor
            .extract("the q3 review went off the rails")
            .await
            .unwrap();
        assert!(
            result.concepts.is_empty(),
            "stub must return no concepts: {result:?}"
        );
        assert!(
            result.edges.is_empty(),
            "stub must return no edges: {result:?}"
        );
    }
}
