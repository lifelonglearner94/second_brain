# Thematic-origin proposals carry a frozen evidence snapshot; endorsement is immutable

A Thematic Inference (ADR-0006) is proposed because the LLM sees thematic density in the current Thematic Read Model partition (ADR-0008) - a cluster of concepts with no connecting edge path. But the partition is non-deterministic and session-scoped: the cluster that motivated the proposal will not exist tomorrow. Once the user endorses the proposal (whether it produces an edge or a concept), it persists as a first-class citizen - but its *reason for existing* can vanish without the proposal itself changing. This is structurally different from Structural Inference, whose evidence (the edge path) *is* the graph and never disappears.

## Decision

The act of clicking "Approve" is an immutable event; the payload of that event includes a **Thematic Snapshot** - a frozen capture of the cluster's composition at proposal time (specifically, the braindump IDs whose edges formed the thematic density). The proposal (edge or concept) lives forever as a first-class citizen, and its provenance metadata holds the historical receipt of the ephemeral cluster that birthed it. The user can always audit, six months later, exactly why they approved a thematic-origin proposal.

The snapshot mechanism extends to any thematic-origin proposal: edges *and* concepts. An asymmetry where thematic edges carried snapshots but thematic concepts did not would recreate the exact "untraceable ghost" problem (rejected below) at the concept layer - a thematic-origin concept like "The Q3 Crisis" would sit in the graph with no receipt, while a thematic-origin edge next to it had a full frozen cluster. This is also the first concrete specification of concept-level provenance, which the Provenance glossary entry references as "backing an edge (and concept)" but had not previously defined.

## Considered options

- **Accept the orphan (no snapshot):** rejected - creates untraceable ghosts. If the user looks at a thematic-origin edge or concept six months later, they cannot know why they approved it; the evidence evaporated. Violates the absolute commitment to Provenance (ADR-0002).
- **Re-validate over time:** rejected - a trap. If the user clicks "Approve" on `Maria -[causes]→ burnout`, that hypothesis ceases to be a statistical guess and becomes an endorsed fact in the brain. Continually re-evaluating an endorsed fact just because the graph's macro-structure shifted creates a self-doubting system and massive alert fatigue. Endorsement is immutable; re-validation undermines it.
- **Edge-only snapshot (asymmetric):** rejected - recreates the "untraceable ghost" problem at the concept layer. A thematic-origin concept like "The Q3 Crisis" would sit in the graph with no receipt while a thematic-origin edge next to it carried a full frozen cluster. The snapshot mechanism must cover all thematic-origin proposals.

## Consequences

- **Refines ADR-0006:** Thematic Inference provenance carries a Thematic Snapshot; Structural Inference provenance does not (its evidence is the graph itself, always present). The two proposal modes now differ not only in epistemic status but in provenance weight. Applies to any thematic-origin proposal - edge or concept.
- **Refines ADR-0002:** provenance entries for thematic inferences include a frozen evidence snapshot - a new field type beyond the origin tag and braindump-id list. Also gives the first concrete specification of concept-level provenance (which the Provenance glossary entry references as "backing an edge (and concept)" but had not previously defined): concept provenance is origin-typed and carries snapshots for thematic-origin concept proposals, symmetric to edges.
- The snapshot is a frozen receipt, never re-evaluated. Endorsement is an immutable event; the Thematic Read Model's continued evolution does not retroactively threaten endorsed edges.
- Thematic Inference provenance is heavier than Structural Inference provenance (carries snapshot data). This is the cost of preserving auditability for edges whose evidence is ephemeral by nature.
