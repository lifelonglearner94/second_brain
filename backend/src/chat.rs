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
use crate::error::Result;
use crate::llm::Llm;
use crate::retrieval::{self, RetrievalResult, RetrievedBraindump, RetrievedEdge};
use crate::thematic::Partition;

/// The exact phrasing ADR-0005 prescribes when the graph cannot support an
/// answer. Lives in the response body verbatim so the frontend can match on it.
pub const SILENCE_MESSAGE: &str = "you haven't told me about that";

/// The cluster-citation marker the LLM is told never to emit. Mirrors the
/// `[bd:<id>]` and `[edge:...]` citation forms so a cluster citation is
/// unambiguous — and so the structural guard can reject it without false
/// positives on prose that merely mentions a "Group N" label.
const CLUSTER_CITATION_MARKER: &str = "[cluster:";

/// The grounded-synthesis system prompt preamble. Appended to the retrieved
/// context (braindumps + edge paths) and the macrostructure partition per call.
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
5. Clusters are a magnifying glass, never a source. The Macrostructure context \
below shows the current thematic partition — ephemeral \"Group N for this \
session\" labels that exist only for this call. You may use it to notice \
thematic structure you could not derive from the raw edges alone. NEVER cite a \
cluster as a source — there is no [cluster:...] citation and there never will \
be. Every claim still cites the underlying braindumps and edges (the stable \
truth), never the ephemeral projection.\n\
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

/// Run the chat read path for a query: retrieve (ADR-0004), layer in the
/// current thematic partition as macrostructure context (ADR-0008), then
/// synthesize (ADR-0005) or fall silent. Reuses the retrieval read path for
/// grounding, the [`Llm`] seam for synthesis, and the Thematic Read Model
/// for the partition.
///
/// When retrieval yields no braindumps, returns the silence response without
/// invoking the LLM or computing the partition — the honesty contract is
/// structural, not prompt-only. Otherwise, builds the grounded-synthesis prompt
/// from the retrieved braindumps + edge paths + the macrostructure partition,
/// calls [`Llm::synthesize`], and returns the answer with the retrieved
/// braindumps and edges as citations.
///
/// Two structural backstops on the LLM output: if the model echoes the silence
/// phrasing (braindumps were retrieved but don't address the query) the
/// response is silent with no citations; and if the model cites a cluster
/// (`[cluster:...]`) — violating ADR-0008's "clusters are a magnifying glass,
/// never a source" — the response is silent with no citations, so the frontend
/// never shows sources for an answer resting on an ephemeral projection.
pub async fn chat(db: &Db, llm: &dyn Llm, query: &str) -> Result<ChatResponse> {
    let retrieved = retrieval::retrieve(db, llm, query).await?;
    if retrieved.braindumps.is_empty() {
        return Ok(silence(retrieved.mode));
    }
    let partition = crate::thematic::partition(db).await?;
    let system = build_synthesis_prompt(&retrieved, &partition);
    let answer = llm.synthesize(&system, query).await?;
    if answer.trim() == SILENCE_MESSAGE {
        return Ok(silence(retrieved.mode));
    }
    if contains_cluster_citation(&answer) {
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

/// Build the grounded-synthesis system prompt from the retrieved context and
/// the current thematic partition. Pure (no I/O) so it is hermetically
/// testable. The preamble states the citation / graph-constrained-inference /
/// silence / no-cluster-cite rules; the appended context lists the
/// macrostructure partition (ADR-0008 — global view), each braindump
/// (id + verbatim — local evidence), and each traversed edge
/// (source —type→ target — local structure).
fn build_synthesis_prompt(retrieved: &RetrievalResult, partition: &Partition) -> String {
    let mut s = String::from(SYNTHESIS_SYSTEM_PREAMBLE);
    s.push_str(
        "\n\nMacrostructure context (current thematic partition — ephemeral, non-citable):\n",
    );
    if partition.clusters.is_empty() {
        s.push_str("(none)\n");
    } else {
        for c in &partition.clusters {
            s.push_str(&format!("[{}] {}\n", c.label, c.concept_labels.join(", ")));
        }
    }
    s.push_str("\nRetrieved braindumps:\n");
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

/// Structural backstop for ADR-0008: detect a cluster citation in the LLM's
/// answer. The LLM is told to cite braindumps as `[bd:<id>]` and edges as
/// `[edge:...]`; a `[cluster:...]` citation would mean the model is resting a
/// claim on the ephemeral projection. Case-insensitive on the marker so
/// `[Cluster:` is caught too. Mentioning a "Group N" label in prose is not a
/// citation and is left alone.
fn contains_cluster_citation(answer: &str) -> bool {
    answer
        .to_ascii_lowercase()
        .contains(CLUSTER_CITATION_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::{BraindumpSource, RetrievalMode, RetrievedBraindump, RetrievedEdge};
    use crate::thematic::{Cluster, Partition};

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

    fn empty_retrieved() -> RetrievalResult {
        RetrievalResult {
            braindumps: vec![],
            paths: vec![],
            mode: RetrievalMode::SeedThenExpand,
        }
    }

    /// ADR-0008 macrostructure context: two clusters with the throwaway session
    /// labels the Thematic Read Model emits.
    fn sample_partition() -> Partition {
        Partition {
            clusters: vec![
                Cluster {
                    label: "Group 1 for this session".to_string(),
                    concept_ids: vec![1, 2],
                    concept_labels: vec!["Maria".to_string(), "Q3 launch".to_string()],
                },
                Cluster {
                    label: "Group 2 for this session".to_string(),
                    concept_ids: vec![3, 4],
                    concept_labels: vec!["Alpha".to_string(), "Beta".to_string()],
                },
            ],
            concept_count: 4,
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
        let prompt = build_synthesis_prompt(&retrieved, &Partition::default());

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
        let prompt = build_synthesis_prompt(&empty_retrieved(), &Partition::default());
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

    #[test]
    fn synthesis_prompt_layers_the_partition_as_macrostructure_context() {
        // ADR-0008: the current partition is layered into chat as
        // macrostructure context — a magnifying glass the LLM uses to see
        // thematic structure it could not derive from raw edges in-budget.
        let retrieved = RetrievalResult {
            braindumps: vec![braindump(42, "maria leaving tanks the timeline")],
            paths: vec![edge("Maria", "endangers", "Q3 launch")],
            mode: RetrievalMode::SeedThenExpand,
        };
        let prompt = build_synthesis_prompt(&retrieved, &sample_partition());
        assert!(
            prompt.contains("Macrostructure context"),
            "macrostructure section header present: {prompt}"
        );
        assert!(
            prompt.contains("Group 1 for this session")
                && prompt.contains("Group 2 for this session"),
            "ADR-0008 ephemeral session labels in the prompt: {prompt}"
        );
        assert!(
            prompt.contains("Maria")
                && prompt.contains("Q3 launch")
                && prompt.contains("Alpha")
                && prompt.contains("Beta"),
            "every concept label appears in the macrostructure: {prompt}"
        );
    }

    #[test]
    fn synthesis_prompt_forbids_citing_clusters_and_requires_braindump_edge_citations() {
        // ADR-0008: clusters are a magnifying glass, never a source. The prompt
        // must reject cluster citations and require braindumps/edges — this is
        // the "citation-from-cluster case rejected" at the prompt-enforcement
        // level, complemented by the structural guard tested below.
        let prompt = build_synthesis_prompt(&empty_retrieved(), &sample_partition());
        let lower = prompt.to_lowercase();
        assert!(
            lower.contains("never cite a cluster"),
            "no-cluster-cite rule stated: {prompt}"
        );
        assert!(
            prompt.contains("braindumps and edges"),
            "every claim must still cite the stable truth: {prompt}"
        );
    }

    #[test]
    fn synthesis_prompt_keeps_the_no_cite_rule_even_when_the_partition_is_empty() {
        // The no-cite rule lives in the preamble so it is enforced on every
        // call, even when the graph has no clusters yet.
        let prompt = build_synthesis_prompt(&empty_retrieved(), &Partition::default());
        assert!(
            prompt.contains("Macrostructure context"),
            "section present even when empty: {prompt}"
        );
        assert!(
            prompt.contains("(none)"),
            "empty macrostructure rendered as none: {prompt}"
        );
        assert!(
            prompt.to_lowercase().contains("never cite a cluster"),
            "no-cite rule present with no clusters: {prompt}"
        );
    }

    #[test]
    fn cluster_citation_guard_detects_the_cluster_marker_only() {
        // The structural backstop: an answer that cites a cluster (mirroring the
        // [bd:]/[edge:] citation form with [cluster:]) is a trust violation.
        // Mentioning a group label in prose is allowed — only the citation
        // marker is rejected.
        assert!(
            contains_cluster_citation(
                "Q3 is at risk [cluster:Group 1] because Maria is leaving [bd:1]"
            ),
            "explicit [cluster:] citation rejected"
        );
        assert!(
            !contains_cluster_citation(
                "Q3 is at risk because Maria is leaving [bd:1] \
                 [edge:Maria —endangers→ Q3 launch]"
            ),
            "a grounded answer with no cluster citation is left alone"
        );
        assert!(
            !contains_cluster_citation("Group 1 looks thematically dense"),
            "mentioning a group label is not a citation"
        );
    }
}
