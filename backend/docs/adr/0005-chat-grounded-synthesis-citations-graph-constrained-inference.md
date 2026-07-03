# Chat is grounded synthesis: mandatory citations, graph-constrained inference, silence when unsupported

Chat answers a query with **grounded synthesis** over the braindumps retrieved by ADR-0004 — never free retelling. Every claim must cite the braindumps and edges that back it. Citations are mandatory: the user can always trace a sentence back to the exact braindump and the graph edge it rests on.

Chat may **infer**, including multi-hop connections the user overlooked — this is the realization of the "interesting connections form over time" goal. Mere reporting would negate the graph's usefulness. But inference carries a strict **burden of proof**: chat may weave the narrative only along edges that *actually exist* in the graph, and must disclose its sources (braindumps/edges) at every step. The graph doesn't just retrieve (ADR-0004) — it constrains what chat can claim.

When the graph doesn't support an answer, chat is **silent** — it says *"you haven't told me about that"* rather than confabulating. A second brain that retells without citing is one hallucinated memory away from worse than no brain, because it feels authoritative; silence is the honesty contract.

## Considered options

- **Free retelling** (standard RAG chat — LLM synthesizes over retrieved context in its own words, no citations): rejected — it discards the provenance investment (ADR-0002/0003) at the output surface, the same "ornamental" trap as parallel-rank at retrieval (ADR-0004). Fluent but unauditable; the LLM can confabulate connections the user never drew.
- **Report-only** (summarize what the user said, no inference): rejected — negates the graph's usefulness. Surfacing non-obvious, multi-hop connections the user overlooked is the system's core value, and that requires inference.

## Consequences

- The typed-edge/ontology/provenance machinery is load-bearing all the way to the output surface: ADR-0004 makes the graph load-bearing for *retrieval*; this ADR makes it load-bearing for what chat can *claim*. The graph constrains chat's permissible inferences to edges that exist.
- Each chat answer carries a citation graph (braindumps + edges) as a first-class part of the response — the frontend must render these as drill-downable sources, not just display prose.
- **Open — does chat write back?** When chat surfaces a multi-hop connection the user overlooked, it is not yet decided whether that connection becomes a new edge in the graph (feeding back through the ADR-0001/0003 governed merge-suggestion queue) or remains a transient inference shown only in that conversation. This is the next thread.
