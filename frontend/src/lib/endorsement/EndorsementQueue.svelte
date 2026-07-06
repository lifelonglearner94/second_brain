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
		{@const isOpen = expanded.has(proposal.id)}
		<li
			class="proposal card"
			class:open={isOpen}
			data-testid={`endorsement-item-${proposal.id}`}
			data-proposal-id={proposal.id}
		>
			<p class="connection" data-testid={`proposed-connection-${proposal.id}`}>
				<span class="node">{conceptLabel(proposal.source_concept_id)}</span>
				<span class="edge">
					<span class="edge-line" aria-hidden="true"></span>
					<span class="edge-type mono">{proposal.proposed_type}</span>
					<span class="edge-arrow" aria-hidden="true">›</span>
				</span>
				<span class="node">{conceptLabel(proposal.target_concept_id)}</span>
			</p>

			{#if proposal.rationale}
				<p class="rationale">{proposal.rationale}</p>
			{/if}

			<button
				type="button"
				class="btn btn-ghost evidence-toggle"
				data-testid={`evidence-toggle-${proposal.id}`}
				aria-expanded={isOpen}
				onclick={() => toggle(proposal.id)}
			>
				<svg
					class="chev"
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
					aria-hidden="true"
				>
					<path d="M9 6l6 6-6 6" />
				</svg>
				{proposal.snapshot
					? 'Based on thematic density'
					: 'Based on existing path'}
			</button>

			{#if isOpen}
				<div class="evidence fade">
					{#if proposal.snapshot}
						<div
							class="evidence-snapshot"
							data-testid={`evidence-snapshot-${proposal.id}`}
						>
							<p class="snapshot-section eyebrow">Braindumps in the cluster</p>
							<ul class="id-list">
								{#each proposal.snapshot.braindump_ids as bdId (bdId)}
									<li class="mono">{bdId}</li>
								{/each}
							</ul>
							<p class="snapshot-section eyebrow">Concepts in the cluster</p>
							<ul class="id-list">
								{#each proposal.snapshot.concept_ids as cid (cid)}
									<li>{conceptLabel(cid)}</li>
								{/each}
							</ul>
						</div>
					{:else}
						<ol
							class="evidence-path"
							data-testid={`evidence-path-${proposal.id}`}
						>
							{#each proposal.evidence_path as hop, i (i)}
								<li class="hop">
									<span class="hop-index mono">{i + 1}</span>
									<span class="node">{conceptLabel(hop.source_concept_id)}</span
									>
									<span class="edge">
										<span class="edge-type mono">{hop.edge_type}</span>
										<span class="edge-arrow" aria-hidden="true">›</span>
									</span>
									<span class="node">{conceptLabel(hop.target_concept_id)}</span
									>
								</li>
							{/each}
						</ol>
					{/if}
				</div>
			{/if}

			<div class="actions">
				<button
					type="button"
					class="btn btn-primary approve"
					data-testid="approve-connection"
					onclick={() => approve(proposal)}>Approve Connection</button
				>
			</div>
		</li>
	{/each}
</ol>

<style>
	.queue {
		list-style: none;
		padding: 0;
		margin: 0;
		display: grid;
		gap: var(--space-3);
	}
	.proposal {
		padding: var(--space-4) var(--space-5);
		display: grid;
		gap: var(--space-3);
		transition:
			border-color var(--dur-1) var(--ease),
			box-shadow var(--dur-1) var(--ease);
	}
	.proposal.open {
		border-color: var(--border-accent);
		box-shadow: var(--shadow-2);
	}
	.connection {
		margin: 0;
		display: flex;
		align-items: center;
		gap: var(--space-2);
		flex-wrap: wrap;
		font-size: var(--fs-16);
		color: var(--fg);
	}
	.connection .node {
		font-weight: 500;
	}
	.edge {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		color: var(--accent);
	}
	.edge-line {
		inline-size: 1.25rem;
		block-size: 1px;
		background: var(--border-accent);
	}
	.edge-type {
		font-size: var(--fs-12);
		color: var(--accent);
		padding: 0.05rem 0.35rem;
		background: var(--accent-soft);
		border: 1px solid var(--border-accent);
		border-radius: var(--radius-sm);
	}
	.edge-arrow {
		color: var(--accent);
		font-size: 1rem;
		line-height: 1;
	}
	.rationale {
		margin: 0;
		font-size: var(--fs-14);
		color: var(--fg-muted);
		line-height: var(--lh-body);
	}
	.evidence-toggle {
		align-self: start;
		padding-inline-start: 0.35rem;
		color: var(--fg-muted);
	}
	.evidence-toggle .chev {
		inline-size: 1rem;
		block-size: 1rem;
		transition: transform var(--dur-2) var(--ease);
	}
	.evidence-toggle[aria-expanded='true'] {
		color: var(--accent);
	}
	.evidence-toggle[aria-expanded='true'] .chev {
		transform: rotate(90deg);
	}
	.evidence {
		padding: var(--space-4);
		border: 1px solid var(--border-hairline);
		border-radius: var(--radius-md);
		background: var(--bg-sunken);
		font-size: var(--fs-13);
		color: var(--fg-muted);
	}
	.evidence-path {
		list-style: none;
		padding: 0;
		margin: 0;
		display: grid;
		gap: var(--space-2);
	}
	.hop {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		flex-wrap: wrap;
	}
	.hop-index {
		inline-size: 1.4rem;
		block-size: 1.4rem;
		display: grid;
		place-items: center;
		color: var(--fg-subtle);
		background: var(--surface-glass);
		border: 1px solid var(--border-hairline);
		border-radius: 50%;
		font-size: var(--fs-12);
	}
	.hop .node {
		color: var(--fg);
	}
	.snapshot-section {
		margin: 0 0 var(--space-2);
		color: var(--fg-subtle);
	}
	.snapshot-section + .id-list {
		margin: 0 0 var(--space-3);
	}
	.id-list {
		list-style: none;
		padding: 0;
		margin: 0;
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-1);
	}
	.id-list li {
		padding: 0.15rem 0.5rem;
		border: 1px solid var(--border-hairline);
		border-radius: var(--radius-sm);
		color: var(--fg-muted);
		font-size: var(--fs-12);
		background: var(--surface-glass);
	}
	.actions {
		display: flex;
		justify-content: flex-start;
	}
</style>
