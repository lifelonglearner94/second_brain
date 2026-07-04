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
});
