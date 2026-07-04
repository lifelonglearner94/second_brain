import { describe, it, expect } from 'vitest';
import { partitionColor, NO_PARTITION } from '../../src/lib/graph/colors';

describe('partitionColor — Louvain cluster coloring (ADR-0008: IDs come from the backend)', () => {
	it('is deterministic: the same partition_id always maps to the same color', () => {
		expect(partitionColor(3)).toBe(partitionColor(3));
	});

	it('distinct partition_ids map to distinct colors so clusters are visually separable', () => {
		expect(partitionColor(0)).not.toBe(partitionColor(1));
		expect(partitionColor(1)).not.toBe(partitionColor(2));
		expect(partitionColor(0)).not.toBe(partitionColor(7));
	});

	it('returns a CSS color the WebGL renderer accepts', () => {
		expect(partitionColor(4)).toMatch(/^#([0-9a-f]{6})$/i);
	});

	it('stays distinct across a wide range of Louvain clusters (prolific user, ~35k concepts)', () => {
		const colors = new Set<number>();
		const seen = new Set<string>();
		for (let i = 0; i < 64; i++) {
			const c = partitionColor(i);
			seen.add(c);
			colors.add(i);
		}
		expect(seen.size).toBe(colors.size);
	});

	it('gives concepts with no partition entry a stable fallback color', () => {
		expect(partitionColor(NO_PARTITION)).toBe(partitionColor(NO_PARTITION));
		expect(partitionColor(NO_PARTITION)).toMatch(/^#([0-9a-f]{6})$/i);
	});
});
