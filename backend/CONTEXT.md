# Backend

The Rust/Axum orchestrator and graph engine. Owns the core data model — braindumps, concepts, and the knowledge graph that emerges from them — plus the extraction and retrieval pipeline that feeds and queries it.

## Language

**Braindump**:
The atomic unit of input — an immutable snapshot of the user's state of mind at a timestamp. Voice-transcribed or typed, user-confirmed at explicit submit, and preserved verbatim as the source of truth; each braindump also has a cleaned, readable rendering (LLM-produced at ingest) shown by default. Edits exist solely for error-correction (STT hallucinations, typos) and overwrite in place — substantive thinking-evolution is never retroactive, it spawns a new braindump and the graph grows additively. (See ADR-0007.)
_Avoid_: thought, idea, memo, entry, note, recording

**Concept**:
A node in the knowledge graph, extracted from one or more braindumps by the LLM. A concept recurs and accretes over time as the same concept appears across many braindumps, forming the hubs and clusters the user navigates.
_Avoid_: entity, node, thought, knoten, topic

**Edge**:
A typed, directional connection between two concepts (`A —[type]→ B`), where the type is drawn from the ontology — never invented by the LLM. One edge per (source, original type, target) accretes: it carries provenance (origin-typed assertions) and survives until its last asserter is removed. (See ADR-0002; type history refined by ADR-0003; provenance origin-typed by ADR-0006.)
_Avoid_: relationship, connection, link, relation

**Provenance**:
The list of assertions backing an edge (and concept), each tagged by origin — a human braindump, or a chat inference. Lets the graph distinguish raw user thoughts from endorsed LLM deductions, and purge the latter if the brain drifts. (Introduced ADR-0002; origin-typed by ADR-0006.)
_Avoid_: source list, attribution, citation list

**Type history**:
An append-only log of the types an edge has worn, event-sourced: index 0 is the LLM's original assertion (immutable), each refactor appends a migrated type, and the edge's current type is the projected state of the last entry — a read model off the log, not a mutable field. Preserves the original assertion the way the verbatim braindump is preserved under its cleaned rendering. (See ADR-0003.)
_Avoid_: type log, retag log, type field

**Ontology**:
A governed, evolving vocabulary of edge types. The LLM draws from it and never coins a type unsanctioned; new types enter through a curated proposal-and-approval process, and the vocabulary grows over time as use reveals types that didn't fit at day zero.
_Avoid_: edge type vocabulary, relation schema, taxonomy

**Ontology refactor**:
The asynchronous background re-classification of existing edges when the ontology evolves (a type added, split, renamed, or merged). Runs out-of-band so ingest isn't blocked; deterministic at Temperature=0 against a pinned model snapshot, and retags via the append-only type history rather than overwriting. (See ADR-0003.)
_Avoid_: retag job, backfill, migration

**Merge suggestion**:
A borderline pair the pipeline cannot confidently identify as the same — of concepts, of edge types, or of chat-inferred edges — surfaced to the user to confirm or reject rather than committed silently. Reused fractally across three layers: concepts (auto-merge >95%, else suggest — ADR-0001), ontology types (>99.5%, else suggest — ADR-0003), and chat write-back (always proposed, never auto-endorsed — ADR-0006).
_Avoid_: pending merge, merge candidate, duplicate suggestion

**Retrieval**:
The read path that answers a query — concept-embeddings seed the entry concept(s), the graph traverses typed edges to expand the neighbourhood, and braindumps collected from the subgraph (plus braindump-embedding backfill) form the context for chat. The graph is load-bearing; vectors are seed and backfill. (See ADR-0004.)
_Avoid_: GraphRAG, hybrid search, fusion, search

**Chat**:
The conversational read surface that answers a query with grounded synthesis over retrieved braindumps — and a governed write-back surface: chat may also *propose* its inferences as new edges, routed through the merge-suggestion queue (ADR-0006) rather than persisted silently. Inferences are constrained to edges that actually exist (or are proposed-via-governance), must disclose sources at every step, and citations are mandatory; when the graph doesn't support an answer, chat is silent. (See ADR-0005; write-back in ADR-0006.)
_Avoid_: GraphRAG chat, assistant, copilot, Q&A

**Chat inference**:
A connection chat proposes as a candidate edge, carrying its own provenance origin (distinct from a braindump). Strictly a proposal — human-gated via the merge-suggestion queue before it persists, so the graph never silently absorbs the LLM's deductions. Once endorsed, the edge's provenance records it as `asserted_by: [Chat_Inference_ID]`, origin-tagged so user thoughts and LLM deductions stay distinguishable. (See ADR-0006.)
_Avoid_: inferred edge, LLM suggestion, auto-edge
