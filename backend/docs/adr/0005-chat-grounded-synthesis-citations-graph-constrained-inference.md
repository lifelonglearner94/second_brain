# Chat is grounded synthesis: mandatory citations, graph-constrained inference, silence when unsupported

Chat answers a query with **grounded synthesis** over the braindumps retrieved by ADR-0004 - never free retelling. Every claim must cite the braindumps and edges that back it. Citations are mandatory: the user can always trace a sentence back to the exact braindump and the graph edge it rests on.

Chat may **infer**, including multi-hop connections the user overlooked - this is the realization of the "interesting connections form over time" goal. Mere reporting would negate the graph's usefulness. But inference carries a strict **burden of proof**: chat may weave the narrative only along edges that *actually exist* in the graph, and must disclose its sources (braindumps/edges) at every step. The graph doesn't just retrieve (ADR-0004) - it constrains what chat can claim.

When the graph doesn't support an answer, chat is **silent** - it says *"you haven't told me about that"* rather than confabulating. A second brain that retells without citing is one hallucinated memory away from worse than no brain, because it feels authoritative; silence is the honesty contract.

## Considered options

- **Free retelling** (standard RAG chat - LLM synthesizes over retrieved context in its own words, no citations): rejected - it discards the provenance investment (ADR-0002/0003) at the output surface, the same "ornamental" trap as parallel-rank at retrieval (ADR-0004). Fluent but unauditable; the LLM can confabulate connections the user never drew.
- **Report-only** (summarize what the user said, no inference): rejected - negates the graph's usefulness. Surfacing non-obvious, multi-hop connections the user overlooked is the system's core value, and that requires inference.

## Consequences

- The typed-edge/ontology/provenance machinery is load-bearing all the way to the output surface: ADR-0004 makes the graph load-bearing for *retrieval*; this ADR makes it load-bearing for what chat can *claim*. The graph constrains chat's permissible inferences to edges that exist.
- Each chat answer carries a citation graph (braindumps + edges) as a first-class part of the response - the frontend must render these as drill-downable sources, not just display prose.
- **Resolved by ADR-0006 - does chat write back?** Yes: chat-proposed inferences route through governed write-back as edge proposals, never persisted silently. See ADR-0006.
- **Refined by ADR-0008 - what counts as a citable source.** The Thematic Read Model (ADR-0008) feeds into Chat as additional context - the current cluster partition acts as a magnifying glass, letting the LLM see thematic macrostructure it couldn't derive from raw edges within the context window. But clusters are ephemeral projections, not stable truth. The LLM may use them to *find* things; it must never *cite* them. Every claim still cites the underlying braindumps and edges. The burden of proof always lies with the stable, fundamental truth, not with the ephemeral projection.
