import { describe, it, expect, vi } from 'vitest';
import {
	renderMarkdown,
	composeAnswer,
	mountCitationChips
} from '../../src/lib/chat/markdown';
import { parseAnswerCitations } from '../../src/lib/chat/citations';

describe('renderMarkdown - formatted markdown output (issue #95)', () => {
	it('renders emphasis as <strong>/<em>', () => {
		const html = renderMarkdown('**bold** and *italic*');
		expect(html).toContain('<strong>bold</strong>');
		expect(html).toContain('<em>italic</em>');
	});

	it('renders headings', () => {
		const html = renderMarkdown('# Title\n\n## Sub');
		expect(html).toContain('<h1>Title</h1>');
		expect(html).toContain('<h2>Sub</h2>');
	});

	it('renders ordered and unordered lists', () => {
		const ul = renderMarkdown('- a\n- b');
		expect(ul).toContain('<ul>');
		expect(ul).toContain('<li>a</li>');
		expect(ul).toContain('<li>b</li>');
		const ol = renderMarkdown('1. first\n2. second');
		expect(ol).toContain('<ol>');
		expect(ol).toContain('<li>first</li>');
	});

	it('renders inline code and code blocks', () => {
		const inline = renderMarkdown('use `x` here');
		expect(inline).toContain('<code>x</code>');
		const block = renderMarkdown('```\nlet x = 1\n```');
		expect(block).toContain('<pre>');
		expect(block).toContain('<code>');
		expect(block).toContain('let x = 1');
	});
});

describe('renderMarkdown - XSS sanitization (issue #95)', () => {
	it('strips <script> tags from LLM output', () => {
		const html = renderMarkdown('hi <script>alert(1)</script> bye');
		expect(html).not.toContain('<script');
		expect(html).not.toContain('alert(1)');
	});

	it('strips disallowed tags like <img> and event handlers', () => {
		const html = renderMarkdown(
			'<img src="x" onerror="alert(1)"> text'
		);
		expect(html).not.toContain('<img');
		expect(html).not.toContain('onerror');
	});

	it('blocks javascript: URLs in links', () => {
		const html = renderMarkdown('[click](javascript:alert(1))');
		expect(html).not.toContain('javascript:');
		expect(html).not.toContain('alert(1)');
	});

	it('keeps safe http links and hardens them with rel/target', () => {
		const html = renderMarkdown('[docs](https://example.com)');
		expect(html).toContain('href="https://example.com"');
		expect(html).toContain('rel="noopener noreferrer"');
		expect(html).toContain('target="_blank"');
	});
});

describe('composeAnswer - markdown + citation composition', () => {
	it('embeds a citation placeholder inline and reports the chip', () => {
		const segments = parseAnswerCitations(
			'Q3 is at risk [bd:42] because Maria is leaving.'
		);
		const { html, chips } = composeAnswer(segments);
		expect(chips).toEqual([{ index: 1, braindumpId: 42 }]);
		expect(html).toMatch(/<p>Q3 is at risk .* because Maria is leaving\.<\/p>/);
		expect(html).toContain('\u27e6cite:42:1\u27e7');
	});

	it('edge markers are gone (parser stripped them before compose)', () => {
		const segments = parseAnswerCitations(
			'risk [bd:42] [edge:Maria -endangers\u2192 Q3 launch].'
		);
		const { html } = composeAnswer(segments);
		expect(html).not.toContain('edge:');
		expect(html).not.toContain('endangers');
	});
});

describe('mountCitationChips - placeholder to live button', () => {
	it('replaces a placeholder text node with a clickable chip button', () => {
		const root = document.createElement('div');
		root.innerHTML =
			'<p>Q3 is at risk \u27e6cite:42:1\u27e7 because.</p>';
		const onOpen = vi.fn();
		mountCitationChips(root, [{ index: 1, braindumpId: 42 }], onOpen);

		const chip = root.querySelector('[data-testid="chat-citation-chip"]');
		expect(chip).not.toBeNull();
		expect(chip?.textContent).toBe('[1]');
		expect(chip?.getAttribute('data-braindump-id')).toBe('42');
		expect(chip?.getAttribute('aria-label')).toBe('Open source 1');

		chip?.dispatchEvent(new window.Event('click', { bubbles: true }));
		expect(onOpen).toHaveBeenCalledWith(42);
	});

	it('leaves a forged placeholder (no matching chip) as literal text', () => {
		const root = document.createElement('div');
		root.innerHTML = '<p>fake \u27e6cite:999:1\u27e7 end</p>';
		mountCitationChips(root, [{ index: 1, braindumpId: 42 }], vi.fn());
		expect(root.querySelector('[data-testid="chat-citation-chip"]')).toBeNull();
		expect(root.textContent).toContain('\u27e6cite:999:1\u27e7');
	});
});
