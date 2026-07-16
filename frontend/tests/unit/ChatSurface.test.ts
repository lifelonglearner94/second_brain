import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { render, fireEvent, waitFor } from '@testing-library/svelte';
import ChatSurface from '../../src/lib/chat/ChatSurface.svelte';
import type { Braindump, ChatResponse } from '../../src/lib/api/client';

const BRAINDUMP: Braindump = {
	id: 42,
	verbatim: 'maria leaving tanks the timeline',
	cleaned: 'Maria leaving tanks the timeline.',
	created_at: 1_700_000_000
};

const GROUNDED: ChatResponse = {
	answer:
		'Q3 launch is at risk because Maria is leaving [bd:42] [edge:Maria -endangers→ Q3 launch].',
	citations: [
		{
			id: 42,
			verbatim: 'maria leaving tanks the timeline',
			cleaned: 'Maria leaving tanks the timeline.',
			created_at: 1_700_000_000,
			score: 1.0,
			source: 'subgraph'
		}
	],
	paths: [
		{
			source_concept_id: 7,
			source_concept_label: 'Maria',
			target_concept_id: 11,
			target_concept_label: 'Q3 launch',
			edge_type: 'endangers'
		}
	],
	silent: false,
	mode: 'seed_then_expand'
};

const SILENT: ChatResponse = {
	answer: 'you haven\u2019t told me about that',
	citations: [],
	paths: [],
	silent: true,
	mode: 'no_seed_fallback'
};

const MARKDOWN: ChatResponse = {
	answer: '**Risk** confirmed for Q3 [bd:42].',
	citations: [
		{
			id: 42,
			verbatim: 'maria leaving tanks the timeline',
			cleaned: 'Maria leaving tanks the timeline.',
			created_at: 1_700_000_000,
			score: 1.0,
			source: 'subgraph'
		}
	],
	paths: [],
	silent: false,
	mode: 'seed_then_expand'
};

const XSS: ChatResponse = {
	answer: '<script>alert(1)</script> safe answer [bd:42].',
	citations: [
		{
			id: 42,
			verbatim: 'maria leaving tanks the timeline',
			cleaned: 'Maria leaving tanks the timeline.',
			created_at: 1_700_000_000,
			score: 1.0,
			source: 'subgraph'
		}
	],
	paths: [],
	silent: false,
	mode: 'seed_then_expand'
};

type ChatApi = {
	chat(query: string): Promise<ChatResponse>;
	getBraindump(id: number): Promise<Braindump>;
	editBraindump(id: number, verbatim: string): Promise<Braindump>;
};

function apiStub(
	chat: ChatApi['chat'],
	getBraindump: ChatApi['getBraindump']
): ChatApi {
	return {
		chat,
		getBraindump,
		editBraindump: vi.fn<ChatApi['editBraindump']>(
			() => new Promise<Braindump>(() => {})
		)
	};
}

async function submitQuery(
	getByTestId: (id: string) => HTMLElement,
	query: string
): Promise<void> {
	const input = getByTestId('chat-query-input') as HTMLInputElement;
	await fireEvent.input(input, { target: { value: query } });
	await fireEvent.click(getByTestId('chat-submit'));
}

describe('ChatSurface - conversational read surface (ADR-0005, backend #10)', () => {
	let chat: ReturnType<typeof vi.fn<ChatApi['chat']>>;
	let getBraindump: ReturnType<typeof vi.fn<ChatApi['getBraindump']>>;

	beforeEach(() => {
		chat = vi.fn<ChatApi['chat']>();
		getBraindump = vi.fn<ChatApi['getBraindump']>();
	});

	it('submits the query to POST /chat and renders the grounded answer with inline citation chips', async () => {
		chat.mockResolvedValue(GROUNDED);
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'is Q3 at risk?');
		expect(chat).toHaveBeenCalledWith('is Q3 at risk?');
		await waitFor(() => expect(getByTestId('chat-answer')).toBeTruthy());
		expect(getByTestId('chat-answer').textContent).toContain(
			'Q3 launch is at risk'
		);
		const chip = getByTestId('chat-citation-chip');
		expect(chip.textContent).toBe('[1]');
		expect(chip.getAttribute('data-braindump-id')).toBe('42');
	});

	it('renders the answer as formatted markdown while keeping citation chips inline and clickable (issue #95)', async () => {
		chat.mockResolvedValue(MARKDOWN);
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId, container } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'is Q3 at risk?');
		await waitFor(() => expect(getByTestId('chat-answer')).toBeTruthy());
		const strong = container.querySelector('strong');
		expect(strong?.textContent).toBe('Risk');
		const chip = await waitFor(() => getByTestId('chat-citation-chip'));
		expect(chip.textContent).toBe('[1]');
		expect(chip.getAttribute('data-braindump-id')).toBe('42');
		await fireEvent.click(chip);
		await waitFor(() =>
			expect(getByTestId('document-modal-cleaned')).toBeTruthy()
		);
		expect(getBraindump).toHaveBeenCalledWith(42);
	});

	it('sanitizes LLM-produced HTML so scripts cannot render, while chips still work (issue #95)', async () => {
		chat.mockResolvedValue(XSS);
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId, container } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'is Q3 at risk?');
		await waitFor(() => expect(getByTestId('chat-answer')).toBeTruthy());
		expect(container.querySelector('script')).toBeNull();
		expect(container.innerHTML).not.toContain('alert(1)');
		const chip = await waitFor(() => getByTestId('chat-citation-chip'));
		await fireEvent.click(chip);
		await waitFor(() =>
			expect(getByTestId('document-modal-cleaned')).toBeTruthy()
		);
	});

	it('clicking a citation chip opens the Document Modal showing the cited braindump\u2019s cleaned text (fetched via GET /braindumps/:id, backend #5)', async () => {
		chat.mockResolvedValue(GROUNDED);
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'is Q3 at risk?');
		await waitFor(() => expect(getByTestId('chat-citation-chip')).toBeTruthy());
		await fireEvent.click(getByTestId('chat-citation-chip'));
		await waitFor(() =>
			expect(getByTestId('document-modal-cleaned')).toBeTruthy()
		);
		expect(getByTestId('document-modal-cleaned').textContent).toBe(
			'Maria leaving tanks the timeline.'
		);
		expect(getBraindump).toHaveBeenCalledWith(42);
	});

	it('a silent response renders Explicit Silence - distinct from blank, loading, and error', async () => {
		chat.mockResolvedValue(SILENT);
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId, queryByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'what is the meaning of life?');
		await waitFor(() =>
			expect(getByTestId('chat-explicit-silence')).toBeTruthy()
		);
		expect(getByTestId('chat-explicit-silence').textContent).toBe(
			'I cannot find graph-supported evidence to answer this.'
		);
		expect(queryByTestId('chat-loading')).toBeNull();
		expect(queryByTestId('chat-error')).toBeNull();
		expect(queryByTestId('chat-answer')).toBeNull();
		expect(queryByTestId('chat-citation-chip')).toBeNull();
		expect(getByTestId('chat-explicit-silence').textContent).not.toContain(
			'you haven\u2019t told me about that'
		);
	});

	it('hides the retrieval traversal path - only cited braindumps surface (no edge/path text rendered)', async () => {
		chat.mockResolvedValue(GROUNDED);
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId, container, queryByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'is Q3 at risk?');
		await waitFor(() => expect(getByTestId('chat-answer')).toBeTruthy());
		expect(queryByTestId('chat-path')).toBeNull();
		expect(container.textContent).not.toContain('endangers');
		expect(container.textContent).not.toContain('Maria -endangers');
	});

	it('renders a loading state while the chat endpoint is in flight', async () => {
		chat.mockReturnValue(new Promise<ChatResponse>(() => {}));
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'is Q3 at risk?');
		expect(getByTestId('chat-loading')).toBeTruthy();
	});

	it('renders an error state when the chat endpoint fails', async () => {
		chat.mockRejectedValue(new Error('POST /chat failed: 503'));
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'is Q3 at risk?');
		await waitFor(() => expect(getByTestId('chat-error')).toBeTruthy());
		expect(getByTestId('chat-error').textContent).toContain('Could not answer');
	});
});

describe('ChatSurface - chat is unavailable offline (ADR-0005, issue #21)', () => {
	let chat: ReturnType<typeof vi.fn<ChatApi['chat']>>;
	let getBraindump: ReturnType<typeof vi.fn<ChatApi['getBraindump']>>;

	beforeEach(() => {
		chat = vi.fn<ChatApi['chat']>();
		getBraindump = vi.fn<ChatApi['getBraindump']>();
	});

	it('renders a chat-offline state and disables the input + submit when online is false', () => {
		chat.mockResolvedValue(GROUNDED);
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump), online: false }
		});
		const offline = getByTestId('chat-offline');
		expect(offline).toBeTruthy();
		expect(offline.textContent).toContain('Chat unavailable offline');
		expect((getByTestId('chat-submit') as HTMLButtonElement).disabled).toBe(
			true
		);
		expect((getByTestId('chat-query-input') as HTMLInputElement).disabled).toBe(
			true
		);
	});

	it('never calls api.chat when offline, even if the form is submitted directly (defense-in-depth)', async () => {
		chat.mockResolvedValue(GROUNDED);
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump), online: false }
		});
		const input = getByTestId('chat-query-input') as HTMLInputElement;
		await fireEvent.input(input, { target: { value: 'is Q3 at risk?' } });
		const form = getByTestId('chat-submit').closest('form') as HTMLFormElement;
		await fireEvent.submit(form);
		expect(chat).not.toHaveBeenCalled();
	});

	it('hides no chat-offline element and keeps the submit enabled when online is true (default)', () => {
		chat.mockResolvedValue(GROUNDED);
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { queryByTestId, getByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump), online: true }
		});
		expect(queryByTestId('chat-offline')).toBeNull();
		expect((getByTestId('chat-submit') as HTMLButtonElement).disabled).toBe(
			false
		);
		expect((getByTestId('chat-query-input') as HTMLInputElement).disabled).toBe(
			false
		);
	});
});

describe('ChatSurface - copy answer to clipboard (issue #96)', () => {
	let chat: ReturnType<typeof vi.fn<ChatApi['chat']>>;
	let getBraindump: ReturnType<typeof vi.fn<ChatApi['getBraindump']>>;
	let writeText: ReturnType<typeof vi.fn>;
	const clipboardDescriptor = Object.getOwnPropertyDescriptor(
		navigator,
		'clipboard'
	);

	function installClipboard(value: unknown): void {
		Object.defineProperty(navigator, 'clipboard', {
			value,
			configurable: true,
			writable: true
		});
	}

	beforeEach(() => {
		chat = vi.fn<ChatApi['chat']>();
		getBraindump = vi.fn<ChatApi['getBraindump']>();
		writeText = vi.fn().mockResolvedValue(undefined);
		installClipboard({ writeText });
	});

	afterEach(() => {
		if (clipboardDescriptor) {
			Object.defineProperty(navigator, 'clipboard', clipboardDescriptor);
		} else {
			installClipboard(undefined);
		}
		vi.useRealTimers();
	});

	it('shows a copy button at the bottom of a non-silent answer', async () => {
		chat.mockResolvedValue(GROUNDED);
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'is Q3 at risk?');
		await waitFor(() => expect(getByTestId('chat-answer')).toBeTruthy());
		const btn = getByTestId('chat-copy-answer');
		expect(btn).toBeTruthy();
		expect(btn.textContent).toBe('Copy');
	});

	it('clicking copy writes the normalized answer to the clipboard and shows Copied feedback', async () => {
		chat.mockResolvedValue(GROUNDED);
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'is Q3 at risk?');
		await waitFor(() => expect(getByTestId('chat-copy-answer')).toBeTruthy());
		await fireEvent.click(getByTestId('chat-copy-answer'));
		await waitFor(() => expect(writeText).toHaveBeenCalled());
		// [bd:42] → [1], [edge:...] stripped — same normalization as the chips
		expect(writeText).toHaveBeenCalledWith(
			'Q3 launch is at risk because Maria is leaving [1].'
		);
		await waitFor(() =>
			expect(getByTestId('chat-copy-answer').textContent).toBe('Copied')
		);
	});

	it('resets the Copied feedback after a timeout', async () => {
		vi.useFakeTimers();
		chat.mockResolvedValue(GROUNDED);
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'is Q3 at risk?');
		await tick();
		await fireEvent.click(getByTestId('chat-copy-answer'));
		await tick();
		expect(writeText).toHaveBeenCalled();
		expect(getByTestId('chat-copy-answer').textContent).toBe('Copied');
		await vi.advanceTimersByTimeAsync(2000);
		await tick();
		expect(getByTestId('chat-copy-answer').textContent).toBe('Copy');
	});

	it('shows no copy button for the silence state', async () => {
		chat.mockResolvedValue(SILENT);
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId, queryByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'what is the meaning of life?');
		await waitFor(() =>
			expect(getByTestId('chat-explicit-silence')).toBeTruthy()
		);
		expect(queryByTestId('chat-copy-answer')).toBeNull();
	});

	it('shows no copy button for the loading state', async () => {
		chat.mockReturnValue(new Promise<ChatResponse>(() => {}));
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId, queryByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'is Q3 at risk?');
		expect(getByTestId('chat-loading')).toBeTruthy();
		expect(queryByTestId('chat-copy-answer')).toBeNull();
	});

	it('shows no copy button for the error state', async () => {
		chat.mockRejectedValue(new Error('POST /chat failed: 503'));
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId, queryByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'is Q3 at risk?');
		await waitFor(() => expect(getByTestId('chat-error')).toBeTruthy());
		expect(queryByTestId('chat-copy-answer')).toBeNull();
	});

	it('does not crash when the clipboard is unavailable (insecure context)', async () => {
		// Simulate an insecure context where navigator.clipboard is undefined.
		installClipboard(undefined);
		chat.mockResolvedValue(GROUNDED);
		getBraindump.mockResolvedValue(BRAINDUMP);
		const { getByTestId } = render(ChatSurface, {
			props: { api: apiStub(chat, getBraindump) }
		});
		await submitQuery(getByTestId, 'is Q3 at risk?');
		await waitFor(() => expect(getByTestId('chat-copy-answer')).toBeTruthy());
		// Clicking must not throw; the fallback silently clears copied state.
		await fireEvent.click(getByTestId('chat-copy-answer'));
		await waitFor(() =>
			expect(getByTestId('chat-copy-answer').textContent).toBe('Copy')
		);
		expect(writeText).not.toHaveBeenCalled();
	});
});
