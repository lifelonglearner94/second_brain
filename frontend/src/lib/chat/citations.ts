export type AnswerSegment =
	| { kind: 'text'; text: string }
	| { kind: 'citation'; index: number; braindumpId: number };

const BD_MARKER = /\[bd:(\d+)\]/g;
const EDGE_MARKER = / ?\[edge:[^\]]*\]/g;

export function parseAnswerCitations(answer: string): AnswerSegment[] {
	const segments: AnswerSegment[] = [];
	const indexByBraindump = new Map<number, number>();
	let nextIndex = 1;

	let cursor = 0;
	let match: RegExpExecArray | null;
	const bd = new RegExp(BD_MARKER);
	while ((match = bd.exec(answer)) !== null) {
		const [full, idStr] = match;
		const start = match.index;
		if (start > cursor) {
			segments.push({ kind: 'text', text: answer.slice(cursor, start) });
		}
		const braindumpId = Number.parseInt(idStr, 10);
		let index = indexByBraindump.get(braindumpId);
		if (index === undefined) {
			index = nextIndex;
			nextIndex += 1;
			indexByBraindump.set(braindumpId, index);
		}
		segments.push({ kind: 'citation', index, braindumpId });
		cursor = start + full.length;
	}
	if (cursor < answer.length) {
		segments.push({ kind: 'text', text: answer.slice(cursor) });
	}

	for (const seg of segments) {
		if (seg.kind === 'text') {
			seg.text = seg.text.replace(EDGE_MARKER, '');
		}
	}

	return collapseEmptyText(segments);
}

function collapseEmptyText(segments: AnswerSegment[]): AnswerSegment[] {
	const out: AnswerSegment[] = [];
	for (const seg of segments) {
		if (seg.kind === 'text') {
			if (seg.text.length === 0) continue;
			const last = out[out.length - 1];
			if (last && last.kind === 'text') {
				last.text += seg.text;
				continue;
			}
		}
		out.push(seg);
	}
	return out;
}
