# Concept provenance is extraction-based, symmetric to edge provenance

The Provenance glossary entry has always claimed to back "an edge (and concept)," but only edge provenance was ever specified — concept-level provenance was a referenced-but-undefined hole. ADR-0009's extension of the Thematic Snapshot to concepts gave the first concrete specification of concept provenance, but only for thematic-origin concept proposals. Braindump-origin concepts — the common case — had no specified provenance mechanics.

## Decision

A concept carries **extraction provenance**: the list of braindumps the LLM extracted it from. This is symmetric to ADR-0002's edge mechanics: deleting a braindump drops its id from every concept's extraction provenance; a concept vanishes when its last extracting braindump is deleted, just as an edge vanishes when its last asserter is removed. The fractal symmetry between concepts and edges (introduced ADR-0001, extended ADR-0002) now extends to provenance mechanics.

## Considered options

- **Edge-derived only (no concept provenance):** rejected — the Provenance glossary entry's "(and concept)" claim would be a lie. A concept's backing braindumps would be derivable only by unioning the provenance of all edges touching it — a query-time computation, not stored. This makes concept deletion undeclared (no stored provenance to drop from) and breaks the purge lever (can't retract a concept's backing without recomputing from edges).
- **Identity-merge provenance only:** rejected — tracks which extracted concepts accreted via ADR-0001 embedding match, but leaves extraction itself untracked. A concept could not tell you which braindumps mentioned it, only which other concepts it merged with. Insufficient for a system committed to provenance as an audit trail.
- **No concept deletion (block to preserve additive growth):** rejected — ADR-0007's "additive" is about not rewriting braindump text, not about the graph never shrinking via deletion. ADR-0002 already accepts edge vanishing on braindump deletion; blocking concept vanishing would create an asymmetry where edges can disappear but concepts can't, and would leave orphan concepts with no extracting braindump still in the graph.

## Consequences

- **Refines ADR-0002:** concept provenance is now specified as extraction-based, symmetric to edge provenance. The "(and concept)" claim in the Provenance glossary entry is no longer a forward promise but a specified mechanic.
- **Refines ADR-0001:** concept identity (accretion via embedding match) and concept provenance (extraction braindumps) are now distinct concepts — a concept accretes identity through merges (ADR-0001) and accretes provenance through new extractions (this ADR). A merged concept's extraction provenance is the union of its merge members'.
- **Consistent with ADR-0007:** "additive growth" means don't rewrite, not never delete. Braindump deletion is a separate operation; concept vanishing on last-extractor deletion is symmetric to edge vanishing on last-asserter deletion, already accepted by ADR-0002.
- **Open — the cascade.** When a concept vanishes (last extracting braindump deleted), all edges where it is an endpoint lose their endpoint. ADR-0002 specifies edge vanishing on *braindump* assertion removal, not on *endpoint* existence. What happens to edges pointing to a vanished concept — cascade delete, orphan dangle, or blocked deletion — is not yet specified.
