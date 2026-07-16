import { marked } from 'marked';
import DOMPurify from 'dompurify';
import type { AnswerSegment } from './citations';

export type CitationChip = {
	index: number;
	braindumpId: number;
};

export type ComposedAnswer = {
	html: string;
	chips: CitationChip[];
};

/**
 * Citation placeholders are wrapped in U+27E6/U+27E7 (mathematical white
 * brackets) so they survive the markdown parser as plain text (none of the
 * characters are markdown syntax or HTML-escaped by `marked`) and are kept as
 * text nodes by DOMPurify. They are later swapped for live chip buttons.
 */
const PLACEHOLDER_OPEN = '\u27e6';
const PLACEHOLDER_CLOSE = '\u27e7';
const PLACEHOLDER_RE = /\u27e6cite:(\d+):(\d+)\u27e7/g;

function chipPlaceholder(braindumpId: number, index: number): string {
	return `${PLACEHOLDER_OPEN}cite:${braindumpId}:${index}${PLACEHOLDER_CLOSE}`;
}

/**
 * Strict allowlist for chat-answer markdown. Only the formatting the grounded
 * synthesis is expected to produce; everything else (scripts, iframes, styles,
 * images, form controls) is stripped by DOMPurify so LLM output can't inject
 * markup.
 */
const SANITIZE_CONFIG = {
	ALLOWED_TAGS: [
		'p',
		'br',
		'hr',
		'h1',
		'h2',
		'h3',
		'h4',
		'h5',
		'h6',
		'ul',
		'ol',
		'li',
		'blockquote',
		'strong',
		'em',
		'del',
		's',
		'code',
		'pre',
		'a',
		'span'
	],
	ALLOWED_ATTR: ['href', 'title'],
	ALLOW_DATA_ATTR: false,
	ALLOW_ARIA_ATTR: false
};

let linkHookInstalled = false;
function installLinkHook(): void {
	if (linkHookInstalled) return;
	if (typeof DOMPurify !== 'undefined' && DOMPurify.addHook) {
		DOMPurify.addHook('afterSanitizeAttributes', (node) => {
			if (node.nodeName === 'A' && node.getAttribute('href')) {
				node.setAttribute('target', '_blank');
				node.setAttribute('rel', 'noopener noreferrer');
			}
		});
		linkHookInstalled = true;
	}
}

function sanitize(html: string): string {
	if (typeof window === 'undefined') {
		return html
			.replace(/&/g, '&amp;')
			.replace(/</g, '&lt;')
			.replace(/>/g, '&gt;');
	}
	installLinkHook();
	return DOMPurify.sanitize(html, SANITIZE_CONFIG) as string;
}

/**
 * Render a markdown string (with citation markers already removed) to
 * sanitized HTML. Safe to insert via `{@html}`.
 */
export function renderMarkdown(text: string): string {
	const parsed = marked.parse(text, { gfm: true, breaks: true });
	const rawHtml = typeof parsed === 'string' ? parsed : '';
	return sanitize(rawHtml);
}

/**
 * Compose a parsed answer (text + citation segments) into a single sanitized
 * HTML string with citation placeholders embedded inline, plus the list of
 * chips that those placeholders stand in for. The caller renders `html` via
 * `{@html}` and then swaps the placeholders for live chip buttons with
 * `mountCitationChips`.
 */
export function composeAnswer(segments: AnswerSegment[]): ComposedAnswer {
	const chips: CitationChip[] = [];
	let markdown = '';
	for (const seg of segments) {
		if (seg.kind === 'text') {
			markdown += seg.text;
		} else {
			markdown += chipPlaceholder(seg.braindumpId, seg.index);
			chips.push({ index: seg.index, braindumpId: seg.braindumpId });
		}
	}
	return { html: renderMarkdown(markdown), chips };
}

/**
 * Walk the live DOM under `root` and replace citation-placeholder text nodes
 * with clickable chip buttons. Only placeholders that correspond to a real
 * chip in `chips` are replaced; any other `⟦cite:…⟧` text (e.g. an LLM
 * trying to forge a chip) is left as literal text.
 */
export function mountCitationChips(
	root: HTMLElement,
	chips: CitationChip[],
	onOpen: (braindumpId: number) => void
): void {
	const valid = new Set(chips.map((c) => chipPlaceholder(c.braindumpId, c.index)));

	const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
	const targets: { node: Text; matches: RegExpExecArray[] }[] = [];
	let textNode = walker.nextNode() as Text | null;
	while (textNode) {
		PLACEHOLDER_RE.lastIndex = 0;
		const matches: RegExpExecArray[] = [];
		let match: RegExpExecArray | null;
		while ((match = PLACEHOLDER_RE.exec(textNode.data)) !== null) {
			matches.push(match);
		}
		if (matches.length > 0) {
			targets.push({ node: textNode, matches });
		}
		textNode = walker.nextNode() as Text | null;
	}

	for (const { node, matches } of targets) {
		const fragment = document.createDocumentFragment();
		let last = 0;
		for (const match of matches) {
			const [full, idStr, idxStr] = match;
			const start = match.index ?? 0;
			if (start > last) {
				fragment.appendChild(
					document.createTextNode(node.data.slice(last, start))
				);
			}
			if (valid.has(full)) {
				const braindumpId = Number.parseInt(idStr, 10);
				const index = Number.parseInt(idxStr, 10);
				const button = document.createElement('button');
				button.type = 'button';
				button.className = 'citation-chip';
				button.setAttribute('data-testid', 'chat-citation-chip');
				button.setAttribute('data-braindump-id', String(braindumpId));
				button.setAttribute('aria-label', `Open source ${index}`);
				button.textContent = `[${index}]`;
				button.addEventListener('click', () => onOpen(braindumpId));
				fragment.appendChild(button);
			} else {
				fragment.appendChild(document.createTextNode(full));
			}
			last = start + full.length;
		}
		if (last < node.data.length) {
			fragment.appendChild(document.createTextNode(node.data.slice(last)));
		}
		node.parentNode?.replaceChild(fragment, node);
	}
}
