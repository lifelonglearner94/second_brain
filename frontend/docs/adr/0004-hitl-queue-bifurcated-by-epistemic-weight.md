# The HITL queue is bifurcated by epistemic weight, not by the backend's fractal shape

The backend reuses one shape - the Merge Suggestion - fractally across three layers (concept merges, ontology type merges, chat-inferred edges; backend ADR-0001/0003/0006) and exposes them through one API. The frontend deliberately bifurcates this into two queues by epistemic weight rather than rendering one unified list. The Housekeeping Queue holds concept- and type-merge confirmations (low epistemic weight - the user arbitrates similarity; verb: "Merge"). The Endorsement Queue holds chat-inferred edges (high epistemic weight - the user arbitrates trust in an LLM deduction; verb: "Approve Connection"). Mixing the two causes cognitive whiplash: merging "Apple" and "Apples" is semantic housekeeping; approving "Maria causes Burnout" is an epistemological leap. They are different mental operations and belong in different queues.

## Considered options

- **Unified queue** (one list of "things the system wants you to confirm,"不分 concept-merges, type-merges, and chat-inferences together): rejected - the backend's fractal reuse is a data-shape decision, not a UX decision. The user's decision-making mode differs by layer: similarity arbitration vs. LLM-trust arbitration. Context-switching between the two inside one list taxes the user cognitively for no benefit.
- **Contextual surfacing** (no dedicated queue; suggestions appear inline where the user encounters them - a concept-merge on clicking a concept, a chat-inference at the end of a chat turn): rejected - fragments the HITL workload across the app and makes it easy to miss pending proposals. A user who doesn't click the right concept never sees the merge suggestion. A dedicated queue ensures nothing falls through.

## Consequences

- The backend's one fractal API maps to two frontend queues. The frontend is responsible for routing each suggestion to the correct queue by its type.
- The action verbs are distinct: "Merge" (Housekeeping Queue) and "Approve Connection" (Endorsement Queue). The UI never uses "Merge" for an inferred edge or "Approve" for a concept duplicate - the verbs encode the epistemic difference.
- Endorsed edges are optimistically merged into the Spatial View-Graph immediately on "Approve Connection," consistent with the action-driven local-merge pattern established in ADR-0002 for braindump ingestion. The next Delta Sync reconciles.
- The Evidence Disclosure pattern (structural path vs. thematic snapshot) lives inside the Endorsement Queue; the Housekeeping Queue has no such disclosure because concept/type merges don't carry inference evidence - they carry similarity scores.
