//! Extraction seam (issue #5 / #6, ADR-0001 / ADR-0002 / ADR-0010).
//!
//! The extractor pulls concepts and edges out of a braindump's verbatim text.
//! The real implementation (`crate::gemini::GeminiClient`) calls Gemini at
//! Temperature=0 with structured output (`responseSchema`) and draws every edge
//! type from the governed ontology — the LLM never invents a type. Identity
//! resolution, accretion, provenance, and type-history are NOT the extractor's
//! job; they live in [`crate::graph`] and run in one atomic transaction
//! (ADR-0001). The cleaner is a separate seam on [`crate::llm::LlmClient`].

use async_trait::async_trait;

use crate::error::Result;

/// A concept the LLM extracted from a braindump. Carries only the surface label;
/// the embedding (identity) and provenance (ADR-0010) are the accretion
/// pipeline's concern, added when the concept is resolved against existing ones.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExtractedConcept {
    pub label: String,
}

/// A typed, directional edge between two extracted concepts. `from_label` /
/// `to_label` reference labels emitted in the same extraction's `concepts`;
/// `type_slug` must be drawn from the ontology (unsanctioned types are rejected
/// downstream in [`crate::graph`]). Provenance and type-history (ADR-0002 /
/// ADR-0003) are the accretion pipeline's concern.
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

/// The extraction seam. The real implementation calls Gemini with structured
/// output and draws edge types from the ontology passed in `ontology_slugs`;
/// the stub returns an empty result so ingest wiring is exercised without
/// network or DB churn.
#[async_trait]
pub trait Extractor: Send + Sync {
    async fn extract(&self, verbatim: &str, ontology_slugs: &[String]) -> Result<ExtractionResult>;
}

/// No-op extractor for tests and the ingest skeleton. Returns no concepts and
/// no edges — never touches the network.
#[derive(Clone, Copy, Debug, Default)]
pub struct FakeExtractor;

#[async_trait]
impl Extractor for FakeExtractor {
    async fn extract(
        &self,
        _verbatim: &str,
        _ontology_slugs: &[String],
    ) -> Result<ExtractionResult> {
        Ok(ExtractionResult::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_extractor_returns_no_concepts_or_edges() {
        let result = FakeExtractor
            .extract("the q3 review went off the rails", &[])
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
