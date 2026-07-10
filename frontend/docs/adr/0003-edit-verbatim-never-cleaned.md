# The user edits the verbatim, never the cleaned rendering

Backend ADR-0007 establishes two texts per braindump: the verbatim (the user's committed input, the source of truth) and a cleaned rendering (an LLM-produced, more readable projection generated at ingest, shown by default). The frontend edit surface populates the input field with the verbatim, never the cleaned. When the user saves an error-correction edit, the backend overwrites the verbatim in place and re-runs the extraction pipeline to regenerate a fresh cleaned rendering. The cleaned rendering is a read-only projection at the UI - it is never editable.

## Considered options

- **Edit the cleaned rendering** (populate the edit field with the text shown by default): rejected - it would collapse the verbatim/cleaned distinction. The cleaned text is an LLM projection of clarity, not the human's actual thought. Letting the user edit it would make the cleaned text the editable surface and demote the verbatim to a frozen artifact nobody touches, contradicting backend ADR-0007's model where the verbatim is the source of truth and edits are error-corrections applied to _it_. The UX convenience of "edit what you see" does not justify collapsing the semantic boundary.
- **Show verbatim by default, edit verbatim**: rejected - the cleaned rendering exists for readability (backend ADR-0007); hiding it by default defeats its purpose. The default view stays cleaned; the edit surface switches to verbatim.

## Consequences

- The UI shows the cleaned rendering by default for readability, with a discrete "View Raw" toggle to inspect the verbatim.
- Clicking "Edit" swaps the displayed text from cleaned to verbatim in the input field. The user is now correcting the raw source of truth, not the LLM's projection of it.
- On save, the frontend sends the edited verbatim to the backend; the backend overwrites the verbatim and re-runs extraction to regenerate the cleaned rendering. The cleaned rendering is always a derived artifact of the current verbatim, never an independently editable text.
- The edit flow has a subtle UX cost: the text the user was reading (cleaned) is not the text the user is now editing (verbatim). This is accepted as the price of semantic integrity - the alternative is letting edits silently rewrite an LLM projection and calling it the user's thought.
