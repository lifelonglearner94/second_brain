import type {
	AuthenticationResponseJSON,
	PublicKeyCredentialCreationOptionsJSON,
	PublicKeyCredentialRequestOptionsJSON,
	RegistrationResponseJSON
} from '@simplewebauthn/browser';

export type Health = {
	ok: boolean;
	db: boolean;
	sqlite_vec: boolean;
};

export type ApiFetch = typeof fetch;

export interface ApiClientOptions {
	baseUrl?: string;
	fetch?: ApiFetch;
}

const DEFAULT_BASE_URL = '/api';

export type RegistrationBegin = {
	challenge: PublicKeyCredentialCreationOptionsJSON;
	state: string;
};

export type RegistrationFinishBody = {
	credential: RegistrationResponseJSON;
	state: string;
};

export type RegistrationFinishOk = { registered: true };

export type LoginBegin = {
	challenge: PublicKeyCredentialRequestOptionsJSON;
	state: string;
};

export type LoginFinishBody = {
	credential: AuthenticationResponseJSON;
	state: string;
};

export type LoginOk = { user_id: string };

export type Me = { user_id: string };

export type LogoutOk = { logged_out: true };

export type RecoverResponse = { error: string; message: string };

export type GraphConcept = {
	id: string;
	label: string;
	created_at: string;
};

export type GraphEdge = {
	id: string;
	source_concept_id: string;
	target_concept_id: string;
	original_type: string;
	current_type: string;
	created_at: string;
};

export type GraphPartition = {
	concept_id: string;
	partition_id: number;
};

export type GlobalTopologySnapshot = {
	concepts: GraphConcept[];
	edges: GraphEdge[];
	partitions: GraphPartition[];
};

export type RetaggedEdge = {
	id: string;
	source_concept_id: string;
	target_concept_id: string;
	original_type: string;
	current_type: string;
};

export type GraphDelta = {
	cursor: number;
	added_concepts: GraphConcept[];
	added_edges: GraphEdge[];
	deleted_concept_ids: string[];
	deleted_edge_ids: string[];
	retagged_edges: RetaggedEdge[];
};

export type LogEntry = {
	timestamp: number;
	level: string;
	target: string;
	message: string;
	fields: unknown;
};

export type LogsResponse = {
	logs: LogEntry[];
	count: number;
	capacity: number;
};

export type BraindumpDto = {
	id: string;
	verbatim: string;
	cleaned: string;
	created_at: string;
};

export type RetrievalMode = 'seed_then_expand' | 'no_seed_fallback';

export type BraindumpSource = 'subgraph' | 'backfill' | 'vector_direct';

export type ChatCitation = {
	id: number;
	verbatim: string;
	cleaned: string;
	created_at: number;
	score: number;
	source: BraindumpSource;
};

export type ChatPath = {
	source_concept_id: number;
	source_concept_label: string;
	target_concept_id: number;
	target_concept_label: string;
	edge_type: string;
};

export type ChatResponse = {
	answer: string;
	citations: ChatCitation[];
	paths: ChatPath[];
	silent: boolean;
	mode: RetrievalMode;
};

export type Braindump = {
	id: number;
	verbatim: string;
	cleaned: string;
	created_at: number;
};

export interface ApiClient {
	getHealth(): Promise<Health>;
	registerBegin(): Promise<RegistrationBegin>;
	registerFinish(body: RegistrationFinishBody): Promise<RegistrationFinishOk>;
	loginBegin(): Promise<LoginBegin>;
	loginFinish(body: LoginFinishBody): Promise<LoginOk>;
	logout(): Promise<LogoutOk>;
	getMe(): Promise<Me>;
	recover(): Promise<RecoverResponse>;
	getGraph(): Promise<GlobalTopologySnapshot>;
	getAdminLogs(limit?: number): Promise<LogsResponse>;
	submitBraindump(verbatim: string): Promise<BraindumpDto>;
	getGraphDelta(since?: number): Promise<GraphDelta>;
	chat(query: string): Promise<ChatResponse>;
	getBraindump(id: number): Promise<Braindump>;
}

function ok(res: Response): boolean {
	return res.status >= 200 && res.status < 300;
}

export function createApiClient(opts: ApiClientOptions = {}): ApiClient {
	const baseUrl = opts.baseUrl ?? DEFAULT_BASE_URL;
	const doFetch = opts.fetch ?? globalThis.fetch;

	async function getJson<T>(path: string, errorLabel: string): Promise<T> {
		const res = await doFetch(`${baseUrl}${path}`, {
			credentials: 'include',
			headers: { accept: 'application/json' }
		});
		if (!ok(res)) {
			throw new Error(`${errorLabel} failed: ${res.status}`);
		}
		return (await res.json()) as T;
	}

	async function postJson<T>(path: string, body: unknown, errorLabel: string): Promise<T> {
		const res = await doFetch(`${baseUrl}${path}`, {
			method: 'POST',
			credentials: 'include',
			headers: { 'content-type': 'application/json', accept: 'application/json' },
			body: JSON.stringify(body)
		});
		if (!ok(res)) {
			throw new Error(`${errorLabel} failed: ${res.status}`);
		}
		return (await res.json()) as T;
	}

	return {
		async getHealth(): Promise<Health> {
			return getJson<Health>('/health', 'GET /health');
		},
		async registerBegin(): Promise<RegistrationBegin> {
			return postJson<RegistrationBegin>('/auth/register/begin', null, 'POST /auth/register/begin');
		},
		async registerFinish(body: RegistrationFinishBody): Promise<RegistrationFinishOk> {
			return postJson<RegistrationFinishOk>('/auth/register/finish', body, 'POST /auth/register/finish');
		},
		async loginBegin(): Promise<LoginBegin> {
			return postJson<LoginBegin>('/auth/login/begin', null, 'POST /auth/login/begin');
		},
		async loginFinish(body: LoginFinishBody): Promise<LoginOk> {
			return postJson<LoginOk>('/auth/login/finish', body, 'POST /auth/login/finish');
		},
		async logout(): Promise<LogoutOk> {
			return postJson<LogoutOk>('/auth/logout', null, 'POST /auth/logout');
		},
		async getMe(): Promise<Me> {
			return getJson<Me>('/me', 'GET /me');
		},
		async recover(): Promise<RecoverResponse> {
			return postJson<RecoverResponse>('/auth/recover', null, 'POST /auth/recover');
		},
		async getGraph(): Promise<GlobalTopologySnapshot> {
			return getJson<GlobalTopologySnapshot>('/graph', 'GET /graph');
		},
		async getAdminLogs(limit?: number): Promise<LogsResponse> {
			const path = limit !== undefined ? `/admin/logs?limit=${limit}` : '/admin/logs';
			return getJson<LogsResponse>(path, 'GET /admin/logs');
		},
		async submitBraindump(verbatim: string): Promise<BraindumpDto> {
			return postJson<BraindumpDto>('/braindumps', { verbatim }, 'POST /braindumps');
		},
		async getGraphDelta(since?: number): Promise<GraphDelta> {
			const path = since !== undefined ? `/graph/delta?since=${since}` : '/graph/delta';
			return getJson<GraphDelta>(path, 'GET /graph/delta');
		},
		async chat(query: string): Promise<ChatResponse> {
			return postJson<ChatResponse>('/chat', { query }, 'POST /chat');
		},
		async getBraindump(id: number): Promise<Braindump> {
			return getJson<Braindump>(`/braindumps/${id}`, 'GET /braindumps/:id');
		}
	};
}
