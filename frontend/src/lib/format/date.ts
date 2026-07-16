/**
 * Format an epoch (Unix seconds, as emitted by the backend's `now_seconds()`)
 * as a human-readable "Month YYYY" string (e.g. "July 2026").
 *
 * The `en-US` locale and `UTC` timezone make the output deterministic and
 * independent of the host's local timezone, keeping the formatted form stable
 * and reusable for the future chat-temporal slice (issue #93).
 *
 * Invalid input (NaN, Infinity, out-of-range) yields an empty string rather
 * than throwing.
 */
export function formatMonthYear(epoch: number): string {
	if (!Number.isFinite(epoch)) return '';
	const date = new Date(epoch * 1000);
	if (Number.isNaN(date.getTime())) return '';
	return new Intl.DateTimeFormat('en-US', {
		month: 'long',
		year: 'numeric',
		timeZone: 'UTC'
	}).format(date);
}
