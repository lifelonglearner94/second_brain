# Ontology governance: fractal tolerance split, event-sourced refactor, pinned model

The ontology reuses the *exact* identity-resolution pattern from ADR-0001 (embedding-match + merge suggestions) — fractally, at the type layer — but with a stricter tolerance. **Tolerance is inversely proportional to blast radius:** a wrong concept merge is local and bounded (one node, fixable by split); a wrong *type* merge is a schema error that corrupts every edge using that type and is propagated by the refactor across the whole graph. So the concept layer auto-merges above 95% confidence (ADR-0001), while the ontology layer auto-merges only above 99.5% — obvious 1:1 type duplicates are filtered automatically, everything riskier goes to a human curation queue.

(An earlier, literal tolerance=0 at the schema layer was rejected as dogmatic: it would queue even 0.99-confidence trivialities for manual review, producing alert fatigue with no safety gain over 99.5%.)

## Refactor mechanics

When a curated ontology change is approved, the async refactor retags existing edges to the new vocabulary. It runs at Temperature=0 against a **pinned model snapshot** (e.g. `gpt-4-0613`), never a `-latest` alias — so retagging is deterministic *and* stable across time; the graph does not shift under the user due to background API model bumps. Temperature=0 alone is insufficient: it is deterministic only within a single model snapshot, not across version changes.

## Edge type history (event-sourced)

Retagging does not overwrite an edge's type. Each edge carries an append-only **type history**: index 0 is the LLM's original assertion (immutable), each refactor appends the migrated type, and the edge's *current type* is the projected state of the last entry — a read model off the log, not a stored field. This applies event sourcing to edge type, preserving the original assertion the way the verbatim braindump is preserved under its cleaned rendering (braindump-as-source-of-truth, extended to the edge layer).

Chosen over:
- **Overwrite** the type on retag — rejected: provenance would then claim the asserting braindump asserted a type it never did, breaking source-of-truth.
- **Original + current (two types)** — rejected: collapses on a second refactor (`affects`→`endangers`→`critically_endangers`); only an append-only log survives repeated migrations.

## Consequences

- **Refines ADR-0002:** an edge's type is now an append-only type history, not a fixed field. Edge identity anchors on the *original* asserted type (index 0) — `(source, original_type, target)` — since the current type is a mutable projection; retagging appends to an edge's history rather than deleting one edge and creating another. ADR-0002's "(source, type, target)" key is read with *original* type.
- The fractal reuse means three embedding collections now live in the vector store: braindump-embeddings (semantic search), concept-embeddings (concept identity, ADR-0001), and type-embeddings (ontology dedup, this ADR).
