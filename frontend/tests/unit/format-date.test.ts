import { describe, it, expect } from 'vitest';
import { formatMonthYear } from '../../src/lib/format/date';

describe('formatMonthYear - epoch (seconds) to "Month YYYY" (issue #93)', () => {
	it('formats a known epoch to "Month YYYY"', () => {
		expect(formatMonthYear(1_700_000_000)).toBe('November 2023');
	});

	it('matches the issue example ("July 2026")', () => {
		expect(formatMonthYear(1_782_864_000)).toBe('July 2026');
	});

	it('formats the Unix epoch as January 1970', () => {
		expect(formatMonthYear(0)).toBe('January 1970');
	});

	it('formats a pre-2000 epoch correctly', () => {
		expect(formatMonthYear(944_006_400)).toBe('December 1999');
	});

	it('formats a post-2030 epoch correctly', () => {
		expect(formatMonthYear(1_930_089_600)).toBe('March 2031');
	});

	it('formats a negative epoch (before 1970) without throwing', () => {
		expect(formatMonthYear(-1)).toBe('December 1969');
	});

	it('is deterministic: the same epoch always yields the same string', () => {
		expect(formatMonthYear(1_700_000_000)).toBe(formatMonthYear(1_700_000_000));
	});

	it('returns an empty string for NaN', () => {
		expect(formatMonthYear(Number.NaN)).toBe('');
	});

	it('returns an empty string for Infinity', () => {
		expect(formatMonthYear(Number.POSITIVE_INFINITY)).toBe('');
		expect(formatMonthYear(Number.NEGATIVE_INFINITY)).toBe('');
	});

	it('returns an empty string for an out-of-range finite epoch', () => {
		expect(formatMonthYear(Number.MAX_SAFE_INTEGER)).toBe('');
	});
});
