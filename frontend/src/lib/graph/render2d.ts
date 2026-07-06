import type { MultiDirectedGraph } from 'graphology';

export const HIGHLIGHT_2D = '#ffffff';

export type SigmaLike = {
	on(event: string, handler: (payload: unknown) => void): unknown;
	setSetting(
		key: 'nodeReducer',
		value:
			| ((
					node: string,
					data: Record<string, unknown>
			  ) => Record<string, unknown>)
			| null
	): unknown;
	refresh(): unknown;
	kill(): unknown;
};

export type SigmaFactory = (
	graph: MultiDirectedGraph,
	container: HTMLElement,
	settings?: Record<string, unknown>
) => SigmaLike | Promise<SigmaLike>;

type SigmaCtor = new (
	graph: MultiDirectedGraph,
	container: HTMLElement,
	settings?: Record<string, unknown>
) => SigmaLike;

const defaultFactory: SigmaFactory = async (graph, container, settings) => {
	const { default: Sigma } = await import('sigma');
	return new (Sigma as unknown as SigmaCtor)(graph, container, settings);
};

export type Render2DOptions = {
	selectedNodeId: string | null;
	onSelectNode: (id: string | null, label: string | null) => void;
	sigmaFactory?: SigmaFactory;
};

export type Render2DHandle = {
	setSelected(id: string | null): void;
	getSelected(): string | null;
	destroy(): void;
};

export async function renderSpatialViewGraph2D(
	container: HTMLElement,
	graph: MultiDirectedGraph,
	options: Render2DOptions
): Promise<Render2DHandle> {
	const factory = options.sigmaFactory ?? defaultFactory;
	const sigma = await factory(graph, container, {
		labelDensity: 0.07,
		labelGridCellSize: 60,
		labelRenderedSizeThreshold: 3,
		defaultEdgeColor: '#46506a',
		defaultEdgeType: 'arrow',
		renderEdgeLabels: true,
		labelColor: { color: '#e6e8ec' },
		labelFont: 'system-ui, sans-serif',
		stagePadding: 24
	});

	let selectedNodeId = options.selectedNodeId;

	sigma.setSetting('nodeReducer', (_node, data) => {
		if (_node === selectedNodeId) {
			return { ...data, color: HIGHLIGHT_2D, highlighted: true };
		}
		return data;
	});

	sigma.on('clickNode', (payload) => {
		const node = (payload as { node: string }).node;
		const label = graph.getNodeAttribute(node, 'label') ?? node;
		selectedNodeId = node;
		options.onSelectNode(node, label);
		sigma.refresh();
	});

	sigma.on('clickStage', () => {
		selectedNodeId = null;
		options.onSelectNode(null, null);
		sigma.refresh();
	});

	return {
		setSelected(id: string | null) {
			selectedNodeId = id;
			sigma.refresh();
		},
		getSelected() {
			return selectedNodeId;
		},
		destroy() {
			sigma.kill();
		}
	};
}
