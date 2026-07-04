import { describe, it, expect, vi } from 'vitest';
import { render, waitFor } from '@testing-library/svelte';
import DocumentModal from '../../src/lib/chat/DocumentModal.svelte';
import type { Braindump } from '../../src/lib/api/client';

const BRAINDUMP: Braindump = {
	id: 42,
	verbatim: 'maria leaving tanks the timeline',
	cleaned: 'Maria leaving tanks the timeline.',
	created_at: 1_700_000_000
};

type BraindumpApi = { getBraindump(id: number): Promise<Braindump> };

function apiStub(getBraindump: BraindumpApi['getBraindump']): BraindumpApi {
	return { getBraindump };
}

describe('DocumentModal — isolated braindump reader (ADR-0003, ADR-0005)', () => {
	it('renders a loading state while the braindump is being fetched', () => {
		const getBraindump = vi.fn<BraindumpApi['getBraindump']>(() => new Promise<Braindump>(() => {}));
		const { getByTestId } = render(DocumentModal, {
			props: { braindumpId: 42, api: apiStub(getBraindump), onClose: vi.fn() }
		});
		expect(getByTestId('document-modal-loading')).toBeTruthy();
	});

	it('fetches GET /braindumps/:id and shows the cleaned text by default', async () => {
		const getBraindump = vi.fn<BraindumpApi['getBraindump']>(async () => BRAINDUMP);
		const { getByTestId, queryByTestId } = render(DocumentModal, {
			props: { braindumpId: 42, api: apiStub(getBraindump), onClose: vi.fn() }
		});
		await waitFor(() => expect(getByTestId('document-modal-cleaned')).toBeTruthy());
		expect(getByTestId('document-modal-cleaned').textContent).toBe(
			'Maria leaving tanks the timeline.'
		);
		expect(getBraindump).toHaveBeenCalledWith(42);
		expect(queryByTestId('document-modal-loading')).toBeNull();
	});

	it('the View Raw toggle swaps the rendered text from cleaned to verbatim', async () => {
		const getBraindump = vi.fn<BraindumpApi['getBraindump']>(async () => BRAINDUMP);
		const { getByTestId, queryByTestId } = render(DocumentModal, {
			props: { braindumpId: 42, api: apiStub(getBraindump), onClose: vi.fn() }
		});
		await waitFor(() => expect(getByTestId('document-modal-cleaned')).toBeTruthy());
		getByTestId('document-modal-toggle-raw').click();
		await waitFor(() => expect(getByTestId('document-modal-verbatim')).toBeTruthy());
		expect(getByTestId('document-modal-verbatim').textContent).toBe(
			'maria leaving tanks the timeline'
		);
		expect(queryByTestId('document-modal-cleaned')).toBeNull();
	});

	it('the close button calls onClose', async () => {
		const getBraindump = vi.fn<BraindumpApi['getBraindump']>(async () => BRAINDUMP);
		const onClose = vi.fn();
		const { getByTestId } = render(DocumentModal, {
			props: { braindumpId: 42, api: apiStub(getBraindump), onClose }
		});
		await waitFor(() => expect(getByTestId('document-modal-cleaned')).toBeTruthy());
		getByTestId('document-modal-close').click();
		expect(onClose).toHaveBeenCalledOnce();
	});

	it('renders a not-found error state when the fetch fails (404)', async () => {
		const getBraindump = vi
			.fn<BraindumpApi['getBraindump']>()
			.mockRejectedValue(new Error('GET /braindumps/:id failed: 404'));
		const { getByTestId } = render(DocumentModal, {
			props: { braindumpId: 9999, api: apiStub(getBraindump), onClose: vi.fn() }
		});
		await waitFor(() => expect(getByTestId('document-modal-error')).toBeTruthy());
		expect(getByTestId('document-modal-error').textContent).toContain('not found');
	});

	it('does not render any graph-navigation control — citations are a reading interaction, not navigation (does not move the Spatial View-Graph camera)', async () => {
		const getBraindump = vi.fn<BraindumpApi['getBraindump']>(async () => BRAINDUMP);
		const { getByTestId, queryByTestId, container } = render(DocumentModal, {
			props: { braindumpId: 42, api: apiStub(getBraindump), onClose: vi.fn() }
		});
		await waitFor(() => expect(getByTestId('document-modal-cleaned')).toBeTruthy());
		expect(queryByTestId('document-modal-focus-concept')).toBeNull();
		expect(container.querySelector('[data-testid="graph-view"]')).toBeNull();
	});
});
