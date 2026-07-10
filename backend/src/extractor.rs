//! Extraction data types (issue #5 / #6, ADR-0001 / ADR-0002 / ADR-0010).
//!
//! The extractor pulls concepts and edges out of a braindump's verbatim text.
//! The real implementation (`crate::gemini::GeminiClient`) calls Gemini at
//! Temperature=0 with structured output (`responseSchema`) and draws every edge
//! type from the governed ontology - the LLM never invents a type. Identity
//! resolution, accretion, provenance, and type-history are NOT the extractor's
//! job; they live in [`crate::graph`] and run in one atomic transaction
//! (ADR-0001). Extraction is one method on the single [`crate::llm::Llm`]
//! seam (issue #39 collapsed the former standalone `Extractor` trait into it).

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
