import { describe, it, expect, vi } from 'vitest';
import { render, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import DocumentModal from '../../src/lib/chat/DocumentModal.svelte';
import type { Braindump } from '../../src/lib/api/client';

const BRAINDUMP: Braindump = {
	id: 42,
	verbatim: 'maria leaving tanks the timeline',
	cleaned: 'Maria leaving tanks the timeline.',
	created_at: 1_700_000_000
};

const EDITED_VERBATIM = 'Maria is leaving, which tanks the timeline.';

const EDITED: Braindump = {
	id: 42,
	verbatim: EDITED_VERBATIM,
	cleaned: 'Maria is leaving, which tanks the timeline.',
	created_at: 1_700_000_000
};

type BraindumpApi = {
	getBraindump(id: number): Promise<Braindump>;
	editBraindump(id: number, verbatim: string): Promise<Braindump>;
};

function apiStub(
	getBraindump: BraindumpApi['getBraindump'],
	editBraindump: BraindumpApi['editBraindump'] = vi.fn<BraindumpApi['editBraindump']>(
		() => new Promise<Braindump>(() => {})
	)
): BraindumpApi {
	return { getBraindump, editBraindump };
}

async function typeInto(getByTestId: (id: string) => HTMLElement, testid: string, value: string) {
	const el = getByTestId(testid) as HTMLTextAreaElement;
	el.value = value;
	el.dispatchEvent(new Event('input', { bubbles: true }));
	await tick();
}

describe('DocumentModal — error-correction edit flow (ADR-0003, ADR-0007)', () => {
	it('Edit populates the input with the verbatim, never the cleaned rendering (ADR-0003)', async () => {
		const getBraindump = vi.fn<BraindumpApi['getBraindump']>(async () => BRAINDUMP);
		const { getByTestId, queryByTestId } = render(DocumentModal, {
			props: { braindumpId: 42, api: apiStub(getBraindump), onClose: vi.fn() }
		});
		await waitFor(() => expect(getByTestId('document-modal-cleaned')).toBeTruthy());
		getByTestId('document-modal-edit').click();
		await waitFor(() => expect(getByTestId('document-modal-edit-input')).toBeTruthy());
		const input = getByTestId('document-modal-edit-input') as HTMLTextAreaElement;
		expect(input.value).toBe(BRAINDUMP.verbatim);
		expect(input.value).not.toBe(BRAINDUMP.cleaned);
		expect(queryByTestId('document-modal-cleaned')).toBeNull();
		expect(queryByTestId('document-modal-toggle-raw')).toBeNull();
	});

	it('Save sends the edited verbatim to backend #5 and re-renders the fresh cleaned returned by the backend (never edited client-side)', async () => {
		const getBraindump = vi.fn<BraindumpApi['getBraindump']>(async () => BRAINDUMP);
		const editBraindump = vi.fn<BraindumpApi['editBraindump']>(async () => EDITED);
		const { getByTestId, queryByTestId } = render(DocumentModal, {
			props: { braindumpId: 42, api: apiStub(getBraindump, editBraindump), onClose: vi.fn() }
		});
		await waitFor(() => expect(getByTestId('document-modal-cleaned')).toBeTruthy());
		getByTestId('document-modal-edit').click();
		await waitFor(() => expect(getByTestId('document-modal-edit-input')).toBeTruthy());
		await typeInto(getByTestId, 'document-modal-edit-input', EDITED_VERBATIM);
		getByTestId('document-modal-save').click();
		await waitFor(() => expect(editBraindump).toHaveBeenCalledWith(42, EDITED_VERBATIM));
		await waitFor(() =>
			expect(getByTestId('document-modal-cleaned').textContent).toBe(EDITED.cleaned)
		);
		expect(getByTestId('document-modal-cleaned').textContent).not.toBe(BRAINDUMP.cleaned);
		expect(queryByTestId('document-modal-edit-input')).toBeNull();
		expect(queryByTestId('document-modal-save')).toBeNull();
		expect(queryByTestId('document-modal-cancel')).toBeNull();
		expect(editBraindump).toHaveBeenCalledOnce();
	});
});

