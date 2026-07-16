import { describe, it, expect } from 'vitest';
import {
	parseAnswerCitations,
	normalizeAnswerForClipboard
} from '../../src/lib/chat/citations';

describe('parseAnswerCitations - inline [bd:<id>] chips, [edge:...] hidden (ADR-0004/0005)', () => {
	it('leaves plain prose with no citations as a single text segment', () => {
		const segs = parseAnswerCitations('Nothing to cite here.');
		expect(segs).toEqual([{ kind: 'text', text: 'Nothing to cite here.' }]);
	});

	it('turns a [bd:<id>] marker into a numbered citation chip (1-based, first appearance)', () => {
		const segs = parseAnswerCitations('Q3 is at risk [bd:42].');
		expect(segs).toEqual([
			{ kind: 'text', text: 'Q3 is at risk ' },
			{ kind: 'citation', index: 1, braindumpId: 42 },
			{ kind: 'text', text: '.' }
		]);
	});

	it('numbers each unique braindump by order of first appearance', () => {
		const segs = parseAnswerCitations(
			'A [bd:7] then B [bd:42] then A again [bd:7].'
		);
		const chips = segs.filter((s) => s.kind === 'citation');
		expect(chips).toEqual([
			{ kind: 'citation', index: 1, braindumpId: 7 },
			{ kind: 'citation', index: 2, braindumpId: 42 },
			{ kind: 'citation', index: 1, braindumpId: 7 }
		]);
	});

	it('hides the [edge:...] traversal-path markers - only cited braindumps surface', () => {
		const segs = parseAnswerCitations(
			'Q3 is at risk [bd:42] [edge:Maria -endangers→ Q3 launch].'
		);
		const rendered = segs
			.map((s) => (s.kind === 'citation' ? `[${s.index}]` : s.text))
			.join('');
		expect(rendered).toBe('Q3 is at risk [1].');
		expect(
			segs.some((s) => s.kind === 'text' && s.text.includes('edge:'))
		).toBe(false);
		expect(
			segs.some((s) => s.kind === 'text' && s.text.includes('endangers'))
		).toBe(false);
	});

	it('parses multi-digit braindump ids', () => {
		const segs = parseAnswerCitations('see [bd:1042].');
		const chip = segs.find((s) => s.kind === 'citation');
		expect(chip).toEqual({ kind: 'citation', index: 1, braindumpId: 1042 });
	});
});

describe('normalizeAnswerForClipboard - clean prose for clipboard copy (issue #96)', () => {
	it('converts [bd:<id>] markers to [1]-style references and strips [edge:...] markers', () => {
		const out = normalizeAnswerForClipboard(
			'Q3 is at risk [bd:42] [edge:Maria -endangers→ Q3 launch].'
		);
		expect(out).toBe('Q3 is at risk [1].');
	});

	it('numbers each unique braindump by order of first appearance', () => {
		const out = normalizeAnswerForClipboard(
			'A [bd:7] then B [bd:42] then A again [bd:7].'
		);
		expect(out).toBe('A [1] then B [2] then A again [1].');
	});

	it('leaves plain prose with no citations unchanged', () => {
		expect(normalizeAnswerForClipboard('Nothing to cite here.')).toBe(
			'Nothing to cite here.'
		);
	});

	it('strips edge markers even when there are no braindump citations', () => {
		const out = normalizeAnswerForClipboard(
			'Prose only [edge:some -path→ here].'
		);
		expect(out).toBe('Prose only.');
	});
});
