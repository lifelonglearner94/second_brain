import { describe, it, expect, vi, beforeEach } from 'vitest';
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
