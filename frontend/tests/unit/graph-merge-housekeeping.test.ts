import { describe, it, expect } from 'vitest';
import { applyConceptMerge, applyTypeMerge } from '../../src/lib/graph/merge';
import type {
	GlobalTopologySnapshot,
	ConceptMergeSuggestion,
	OntologyTypeProposal
} from '../../src/lib/api/client';

const SNAPSHOT: GlobalTopologySnapshot = {
	concepts: [
		{ id: '1', label: 'sleep', created_at: '2026-07-01T00:00:00Z' },
		{ id: '2', label: 'Apples', created_at: '2026-07-03T00:00:00Z' },
		{ id: '3', label: 'caffeine', created_at: '2026-07-02T00:00:00Z' }
	],
	edges: [
		{
			id: 'e1',
			source_concept_id: '2',
			target_concept_id: '1',
			original_type: 'affects',
			current_type: 'affects',
			created_at: '2026-07-03T00:00:00Z'
		},
		{
			id: 'e2',
			source_concept_id: '3',
			target_concept_id: '2',
			original_type: 'disrupts',
			current_type: 'disrupts',
			created_at: '2026-07-03T00:00:00Z'
		}
	],
	partitions: [
		{ concept_id: '1', partition_id: 0 },
		{ concept_id: '2', partition_id: 1 },
		{ concept_id: '3', partition_id: 1 }
	]
};

const CONCEPT_SUGGESTION: ConceptMergeSuggestion = {
	id: 1,
	kind: 'concept',
	braindump_id: 5,
	new_concept_label: 'Apples',
	new_concept_id: 2,
	existing_concept_id: 1,
	similarity: 0.92,
	status: 'pending',
	created_at: 1_700_000_000
};

describe('applyConceptMerge - action-driven local-merge of a confirmed concept pair into the Spatial View-Graph (ADR-0002)', () => {
	it('drops the folded (new) concept and keeps the existing one as the survivor', () => {
		const merged = applyConceptMerge(SNAPSHOT, CONCEPT_SUGGESTION);
		expect(merged.concepts.map((c) => c.id).sort()).toEqual(['1', '3']);
		expect(merged.concepts.find((c) => c.id === '1')?.label).toBe('sleep');
	});

	it('repoints edges touching the folded concept onto the survivor', () => {
		const merged = applyConceptMerge(SNAPSHOT, CONCEPT_SUGGESTION);
		const repointed = merged.edges.find((e) => e.id === 'e2');
		expect(repointed?.source_concept_id).toBe('3');
		expect(repointed?.target_concept_id).toBe('1');
		expect(repointed?.current_type).toBe('disrupts');
	});

	it('drops self-loops that collapse onto the survivor (backend merges duplicates; the view drops the loop)', () => {
		const merged = applyConceptMerge(SNAPSHOT, CONCEPT_SUGGESTION);
		expect(merged.edges.find((e) => e.id === 'e1')).toBeUndefined();
		expect(merged.edges).toHaveLength(1);
	});

	it('drops the folded concept partition entry', () => {
		const merged = applyConceptMerge(SNAPSHOT, CONCEPT_SUGGESTION);
		expect(merged.partitions.find((p) => p.concept_id === '2')).toBeUndefined();
		expect(merged.partitions).toHaveLength(2);
	});

	it('does not mutate the input snapshot (the Spatial View-Graph is replaced, not patched in place)', () => {
		applyConceptMerge(SNAPSHOT, CONCEPT_SUGGESTION);
		expect(SNAPSHOT.concepts).toHaveLength(3);
		expect(SNAPSHOT.edges).toHaveLength(2);
		expect(SNAPSHOT.partitions).toHaveLength(3);
		expect(SNAPSHOT.concepts.find((c) => c.id === '2')?.label).toBe('Apples');
	});

	it('is a no-op when the folded concept is absent from the snapshot (defensive against partial views)', () => {
		const partial: GlobalTopologySnapshot = {
			concepts: [{ id: '1', label: 'sleep', created_at: 't' }],
			edges: [],
			partitions: [{ concept_id: '1', partition_id: 0 }]
		};
		const merged = applyConceptMerge(partial, CONCEPT_SUGGESTION);
		expect(merged.concepts.map((c) => c.id)).toEqual(['1']);
	});
});

const TYPE_APPROVAL: OntologyTypeProposal = {
	id: 3,
	slug: 'endangers',
	label: 'Endangers',
	description: 'Causes harm to.',
	merge_of: 'affects',
	status: 'approved',
	near_match_slug: 'affects',
	near_match_similarity: 0.88
};

describe('applyTypeMerge - optimistic edge retag when an ontology type merge is confirmed (backend #3 refactor mirrored locally)', () => {
	it('retags edges whose current_type is the merge_of slug to the new approved slug', () => {
		const merged = applyTypeMerge(SNAPSHOT, TYPE_APPROVAL);
		const retagged = merged.edges.find((e) => e.id === 'e1');
		expect(retagged?.current_type).toBe('endangers');
		expect(retagged?.original_type).toBe('affects');
		expect(merged.edges.find((e) => e.id === 'e2')?.current_type).toBe(
			'disrupts'
		);
	});

	it('leaves concepts and partitions untouched (a type merge only relabels edges)', () => {
		const merged = applyTypeMerge(SNAPSHOT, TYPE_APPROVAL);
		expect(merged.concepts).toEqual(SNAPSHOT.concepts);
		expect(merged.partitions).toEqual(SNAPSHOT.partitions);
	});

	it('does not mutate the input snapshot', () => {
		applyTypeMerge(SNAPSHOT, TYPE_APPROVAL);
		expect(SNAPSHOT.edges.find((e) => e.id === 'e1')?.current_type).toBe(
			'affects'
		);
	});

	it('is a no-op on the snapshot when the approved proposal has no merge_of (pure new type - no edges to retag)', () => {
		const pureNew: OntologyTypeProposal = { ...TYPE_APPROVAL, merge_of: null };
		const merged = applyTypeMerge(SNAPSHOT, pureNew);
		expect(merged).toEqual(SNAPSHOT);
	});
});
