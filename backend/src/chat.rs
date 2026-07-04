//! The chat read surface (issue #10, ADR-0005).
//!
//! Grounded synthesis over retrieved braindumps — never free retelling. The
//! query runs the retrieval read path (ADR-0004); the retrieved braindumps +
//! traversed edge paths become the context for synthesis under a system prompt
//! that enforces three rules:
//!
//! 1. **Mandatory citations** — every claim cites the braindump ids and edge
//!    refs it rests on, so the user can trace any sentence to its source.
//! 2. **Graph-constrained inference** — chat may infer multi-hop connections
//!    the user overlooked, but only along edges that actually exist. The graph
//!    doesn't just retrieve; it constrains what chat can claim.
//! 3. **Silence when unsupported** — when the graph doesn't support an answer,
//!    chat says "you haven't told me about that" rather than confabulating.
//!
//! Silence is enforced structurally as well as in the prompt: when retrieval
//! returns no braindumps, the endpoint returns the silence response without
//! calling the LLM. The honesty contract is not entrusted to the model — a
//! second brain that retells without citing is one hallucinated memory away
//! from worse than no brain, because it feels authoritative.

use serde::Serialize;

use crate::db::Db;
use crate::embedding::EmbeddingClient;
use crate::error::Result;
use crate::llm::LlmClient;
use crate::retrieval::{self, RetrievalResult, RetrievedBraindump, RetrievedEdge};

/// The exact phrasing ADR-0005 prescribes when the graph cannot support an
/// answer. Lives in the response body verbatim so the frontend can match on it.
pub const SILENCE_MESSAGE: &str = "you haven't told me about that";

/// The grounded-synthesis system prompt preamble. Appended to the retrieved
/// context (braindumps + edge paths) per call.
const SYNTHESIS_SYSTEM_PREAMBLE: &str = "\
You are a second-brain. You answer the user's query by synthesizing over the \
retrieved braindumps and the typed-edge graph paths provided below. You never \
retell freehand.\n\
Rules:\n\
1. Grounded synthesis only. Every claim must rest on a braindump or an edge in \
the provided context. Do not draw on outside knowledge.\n\
2. Cite your sources. Every claim must reference the braindump id it rests on \
(in the form [bd:<id>]) and, where a connection is involved, the edge ref it \
traverses (in the form [edge:<source> —<type>→ <target>]). No claim without a \
citation.\n\
3. Graph-constrained inference. You may infer multi-hop connections the user \
overlooked, but ONLY along edges that appear in the provided context. Never \
invent an edge that is not listed below.\n\
4. Silence when unsupported. If the provided context does not support an \
answer to the query, respond with exactly: \"you haven't told me about that\" \
and nothing else.\n\
Respond with the synthesis prose only — no preamble, no meta-commentary.";

/// The chat response. Carries the synthesized answer plus the citations (the
/// retrieved braindumps + edge paths) the answer rests on, so the frontend can
/// render them as drill-downable sources (ADR-0005). `silent` is `true` exactly
/// when the graph could not support an answer — the answer is then
/// [`SILENCE_MESSAGE`] and `citations`/`paths` are empty.
#[derive(Debug, Clone, Serialize)]
pub struct ChatResponse {
    pub answer: String,
    pub citations: Vec<RetrievedBraindump>,
    pub paths: Vec<RetrievedEdge>,
    pub silent: bool,
    pub mode: crate::retrieval::RetrievalMode,
}

/// Run the chat read path for a query: retrieve (ADR-0004), then synthesize
/// (ADR-0005) or fall silent. Reuses the retrieval read path for grounding and
/// the [`LlmClient`] seam for synthesis.
///
/// When retrieval yields no braindumps, returns the silence response without
/// invoking the LLM — the honesty contract is structural, not prompt-only.
/// Otherwise, builds the grounded-synthesis prompt from the retrieved
/// braindumps + edge paths, calls [`LlmClient::synthesize`], and returns the
/// answer with the retrieved braindumps and edges as citations.
pub async fn chat(
    db: &Db,
    embedding: &(dyn EmbeddingClient + Sync),
    llm: &dyn LlmClient,
    query: &str,
) -> Result<ChatResponse> {
    let retrieved = retrieval::retrieve(db, embedding, query).await?;
    if retrieved.braindumps.is_empty() {
        return Ok(silence(retrieved.mode));
    }
    let system = build_synthesis_prompt(&retrieved);
    let answer = llm.synthesize(&system, query).await?;
    // The system prompt instructs the model to echo SILENCE_MESSAGE when the
    // context doesn't support an answer (retrieval returned braindumps, but
    // they don't address the query). Treat that as silence too — the honesty
    // contract must hold whether silence was structural (no braindumps) or
    // LLM-judged (braindumps, but no support), so the frontend never shows
    // citations for an answer that doesn't exist.
    if answer.trim() == SILENCE_MESSAGE {
        return Ok(silence(retrieved.mode));
    }
    Ok(ChatResponse {
        answer,
        citations: retrieved.braindumps,
        paths: retrieved.paths,
        silent: false,
        mode: retrieved.mode,
    })
}

/// Build the silence response: the ADR-0005 phrasing, no citations, no paths.
/// The retrieval mode is preserved so the frontend can still report how the
/// (empty) result set was reached.
fn silence(mode: crate::retrieval::RetrievalMode) -> ChatResponse {
    ChatResponse {
        answer: SILENCE_MESSAGE.to_string(),
        citations: Vec::new(),
        paths: Vec::new(),
        silent: true,
        mode,
    }
}

/// Build the grounded-synthesis system prompt from the retrieved context. Pure
/// (no I/O) so it is hermetically testable. The preamble states the citation /
/// graph-constrained-inference / silence rules; the appended context lists each
/// braindump (id + verbatim) and each traversed edge (source —type→ target).
fn build_synthesis_prompt(retrieved: &RetrievalResult) -> String {
    let mut s = String::from(SYNTHESIS_SYSTEM_PREAMBLE);
    s.push_str("\n\nRetrieved braindumps:\n");
    if retrieved.braindumps.is_empty() {
        s.push_str("(none)\n");
    } else {
        for b in &retrieved.braindumps {
            s.push_str(&format!("[bd:{}] {}\n", b.id, b.cleaned));
        }
    }
    s.push_str("\nTraversed edge paths:\n");
    if retrieved.paths.is_empty() {
        s.push_str("(none)\n");
    } else {
        for e in &retrieved.paths {
            s.push_str(&format!(
                "{} —{}→ {}\n",
                e.source_concept_label, e.edge_type, e.target_concept_label
            ));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::{BraindumpSource, RetrievalMode, RetrievedBraindump, RetrievedEdge};

    fn braindump(id: i64, cleaned: &str) -> RetrievedBraindump {
        RetrievedBraindump {
            id,
            verbatim: cleaned.to_string(),
            cleaned: cleaned.to_string(),
            created_at: 0,
            score: 1.0,
            source: BraindumpSource::Subgraph,
        }
    }

    fn edge(src: &str, etype: &str, tgt: &str) -> RetrievedEdge {
        RetrievedEdge {
            source_concept_id: 1,
            source_concept_label: src.to_string(),
            target_concept_id: 2,
            target_concept_label: tgt.to_string(),
            edge_type: etype.to_string(),
        }
    }

    #[test]
    fn silence_response_carries_the_adr_phrasing_and_no_citations() {
        let resp = silence(RetrievalMode::SeedThenExpand);
        assert!(resp.silent);
        assert_eq!(resp.answer, SILENCE_MESSAGE);
        assert!(resp.citations.is_empty());
        assert!(resp.paths.is_empty());
    }

    #[test]
    fn silence_preserves_the_retrieval_mode_for_frontend_visibility() {
        let resp = silence(RetrievalMode::NoSeedFallback);
        assert_eq!(resp.mode, RetrievalMode::NoSeedFallback);
    }

    #[test]
    fn synthesis_prompt_lists_each_braindump_id_and_edge_for_the_llm_to_cite() {
        // The citation/graph-constraint rules are load-bearing only if the
        // prompt actually carries the braindump ids + edge refs the LLM must
        // cite. If the prompt dropped them, "mandatory citations" would be
        // unenforceable — the model would have nothing to point at.
        let retrieved = RetrievalResult {
            braindumps: vec![braindump(42, "maria leaving tanks the timeline")],
            paths: vec![edge("Maria", "endangers", "Q3 launch")],
            mode: RetrievalMode::SeedThenExpand,
        };
        let prompt = build_synthesis_prompt(&retrieved);

        assert!(
            prompt.contains("[bd:42]"),
            "braindump id citable in the prompt: {prompt}"
        );
        assert!(
            prompt.contains("maria leaving tanks the timeline"),
            "braindump text present for grounding: {prompt}"
        );
        assert!(
            prompt.contains("Maria —endangers→ Q3 launch"),
            "edge ref citable in the prompt: {prompt}"
        );
    }

    #[test]
    fn synthesis_prompt_states_the_citation_and_silence_rules() {
        let retrieved = RetrievalResult {
            braindumps: vec![],
            paths: vec![],
            mode: RetrievalMode::SeedThenExpand,
        };
        let prompt = build_synthesis_prompt(&retrieved);
        assert!(
            prompt.contains("Cite your sources"),
            "citation rule stated: {prompt}"
        );
        assert!(
            prompt.contains("Graph-constrained inference"),
            "graph-constraint rule stated: {prompt}"
        );
        assert!(
            prompt.contains("Silence when unsupported"),
            "silence rule stated: {prompt}"
        );
        assert!(
            prompt.contains(SILENCE_MESSAGE),
            "silence phrasing present for the LLM to echo: {prompt}"
        );
    }
}
