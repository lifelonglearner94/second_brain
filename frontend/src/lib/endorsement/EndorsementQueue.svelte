<script lang="ts">
	import type { ChatInferenceProposal } from '$lib/api/client';

	let {
		proposals,
		labelFor,
		onApproveConnection
	}: {
		proposals: ChatInferenceProposal[];
		labelFor: (conceptId: number) => string | null;
		onApproveConnection: (
			proposal: ChatInferenceProposal
		) => Promise<void> | void;
	} = $props();

	let expanded = $state<Set<number>>(new Set());

	function toggle(id: number): void {
		const next = new Set(expanded);
		if (next.has(id)) {
			next.delete(id);
		} else {
			next.add(id);
		}
		expanded = next;
	}

	function conceptLabel(id: number): string {
		return labelFor(id) ?? String(id);
	}

	async function approve(proposal: ChatInferenceProposal): Promise<void> {
		await onApproveConnection(proposal);
	}
</script>

<ol class="queue" data-testid="endorsement-queue-list">
	{#each proposals as proposal (proposal.id)}
		<li
			class="proposal"
			data-testid={`endorsement-item-${proposal.id}`}
			data-proposal-id={proposal.id}
		>
			<p class="connection" data-testid={`proposed-connection-${proposal.id}`}>
				<span class="node">{conceptLabel(proposal.source_concept_id)}</span>
				<span class="edge">—[{proposal.proposed_type}]→</span>
				<span class="node">{conceptLabel(proposal.target_concept_id)}</span>
			</p>

			{#if proposal.rationale}
				<p class="rationale">{proposal.rationale}</p>
			{/if}

			<button
				type="button"
				class="evidence-toggle"
				data-testid={`evidence-toggle-${proposal.id}`}
				aria-expanded={expanded.has(proposal.id)}
				onclick={() => toggle(proposal.id)}
			>
				{proposal.snapshot
					? 'Based on thematic density'
					: 'Based on existing path'}
			</button>

			{#if expanded.has(proposal.id)}
				{#if proposal.snapshot}
					<div
						class="evidence snapshot"
						data-testid={`evidence-snapshot-${proposal.id}`}
					>
						<p class="snapshot-section">Braindumps in the cluster</p>
						<ul class="id-list">
							{#each proposal.snapshot.braindump_ids as bdId (bdId)}
								<li>{bdId}</li>
							{/each}
						</ul>
						<p class="snapshot-section">Concepts in the cluster</p>
						<ul class="id-list">
							{#each proposal.snapshot.concept_ids as cid (cid)}
								<li>{conceptLabel(cid)}</li>
							{/each}
						</ul>
					</div>
				{:else}
					<ol
						class="evidence path"
						data-testid={`evidence-path-${proposal.id}`}
					>
						{#each proposal.evidence_path as hop, i (i)}
							<li>
								<span class="node">{conceptLabel(hop.source_concept_id)}</span>
								<span class="edge">—[{hop.edge_type}]→</span>
								<span class="node">{conceptLabel(hop.target_concept_id)}</span>
							</li>
						{/each}
					</ol>
				{/if}
			{/if}

			<button
				type="button"
				class="approve"
				data-testid="approve-connection"
				onclick={() => approve(proposal)}
			>
				Approve Connection
			</button>
		</li>
	{/each}
</ol>

<style>
	.queue {
		list-style: none;
		padding: 0;
		margin: 0;
		display: grid;
		gap: 0.75rem;
	}
	.proposal {
		padding: 0.75rem 0.9rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.5rem;
		background: #11141b;
		display: grid;
		gap: 0.5rem;
	}
	.connection {
		margin: 0;
		font-size: 1rem;
		color: #e6e8ec;
	}
	.connection .edge {
		color: #7ab7ff;
		font-family: monospace;
		margin: 0 0.25rem;
	}
	.rationale {
		margin: 0;
		font-size: 0.9rem;
		color: #9aa3b2;
	}
	.evidence-toggle {
		align-self: start;
		padding: 0.35rem 0.6rem;
		font-size: 0.85rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.4rem;
		background: #1a1f2b;
		color: #c4cbd6;
		cursor: pointer;
	}
	.evidence-toggle[aria-expanded='true'] {
		border-color: #7ab7ff;
		color: #7ab7ff;
	}
	.evidence {
		padding: 0.5rem 0.65rem;
		border: 1px solid #1f242e;
		border-radius: 0.4rem;
		background: #0b0d12;
		font-size: 0.85rem;
		color: #c4cbd6;
	}
	.path {
		list-style: none;
		padding: 0;
		margin: 0;
		display: grid;
		gap: 0.25rem;
	}
	.path .edge {
		color: #7ab7ff;
		font-family: monospace;
		margin: 0 0.2rem;
	}
	.snapshot-section {
		margin: 0 0 0.25rem;
		color: #9aa3b2;
		font-weight: 600;
	}
	.snapshot-section + .id-list {
		margin: 0 0 0.5rem;
	}
	.id-list {
		list-style: none;
		padding: 0;
		margin: 0;
		display: flex;
		flex-wrap: wrap;
		gap: 0.3rem;
	}
	.id-list li {
		font-family: monospace;
		padding: 0.1rem 0.4rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.3rem;
		color: #c4cbd6;
	}
	.approve {
		align-self: start;
		padding: 0.5rem 0.9rem;
		font-size: 0.95rem;
		border: 1px solid #7ab7ff;
		border-radius: 0.5rem;
		background: #1a1f2b;
		color: #7ab7ff;
		cursor: pointer;
	}
	.approve:hover {
		background: #243049;
	}
</style>
