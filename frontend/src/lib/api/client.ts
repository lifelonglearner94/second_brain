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
	// The backend returns the WebAuthn-spec `CreationChallengeResponse`,
	// which wraps the bare creation options under `publicKey`. The frontend
	// unwraps before handing the bare options to `startRegistration`.
	challenge: { publicKey: PublicKeyCredentialCreationOptionsJSON };
	state: string;
};

export type RegistrationFinishBody = {
	credential: RegistrationResponseJSON;
	state: string;
};

export type RegistrationFinishOk = { registered: true; user_id: string };

export type LoginBegin = {
	// The backend returns the WebAuthn-spec `RequestChallengeResponse`,
	// which wraps the bare request options under `publicKey`. The frontend
	// unwraps before handing the bare options to `startAuthentication`.
	challenge: { publicKey: PublicKeyCredentialRequestOptionsJSON };
	state: string;
};

export type LoginFinishBody = {
	credential: AuthenticationResponseJSON;
	state: string;
};

export type LoginOk = { user_id: string };

export type Me = { user_id: string; display_name: string; is_admin: boolean };

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

// Issue #81: host load metrics from GET /admin/system. Raw bytes are surfaced
// so the frontend formats human-friendly sizes; the backend owns no formatting.
export type CpuMetrics = {
	usage_percent: number;
	cores: number;
	per_core: number[];
};

export type MemoryMetrics = {
	total_bytes: number;
	used_bytes: number;
	usage_percent: number;
};

export type DiskMetrics = {
	name: string;
	mount_point: string;
	total_bytes: number;
	used_bytes: number;
	usage_percent: number;
};

export type SystemResponse = {
	cpu: CpuMetrics;
	memory: MemoryMetrics;
	disks: DiskMetrics[];
	// Mount point of the filesystem holding the Brain File (SQLite db), so the
	// page can highlight the right disk. null when the db is in-memory or the
	// mount can't be resolved.
	brain_file_mount: string | null;
};

export type Invitation = {
	id: number;
	token: string;
	created_by_user_id: string;
	status: string;
	created_at: number;
	consumed_at: number | null;
	consumed_by_user_id: string | null;
	consumed_by_display_name: string | null;
};

export type InvitationsResponse = {
	invitations: Invitation[];
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

export type IngestStatus = {
	status: string;
	attempts: number;
	last_attempt_at: number | null;
};

export type ConceptMergeSuggestion = {
	id: number;
	kind: string;
	braindump_id: number;
	new_concept_label: string;
	new_concept_id: number;
	existing_concept_id: number;
	existing_concept_label: string;
	braindump_snippet: string;
	similarity: number;
	status: string;
	created_at: number;
};

export type OntologyEdgeType = {
	slug: string;
	label: string;
	description: string;
};

export type Ontology = {
	edge_types: OntologyEdgeType[];
};

export type OntologyTypeProposal = {
	id: number;
	slug: string;
	label: string;
	description: string;
	merge_of: string | null;
	status: string;
	near_match_slug: string | null;
	near_match_similarity: number | null;
};

export type OntologyProposalsResponse = {
	proposals: OntologyTypeProposal[];
};

export type EvidenceEdge = {
	source_concept_id: number;
	edge_type: string;
	target_concept_id: number;
};

export type ThematicSnapshot = {
	id: number;
	braindump_ids: number[];
	concept_ids: number[];
	captured_at: number;
};

export type ChatInferenceProposal = {
	id: number;
	mode: string;
	source_concept_id: number;
	target_concept_id: number;
	proposed_type: string;
	evidence_path: EvidenceEdge[];
	rationale: string | null;
	status: string;
	created_at: number;
	resolved_at: number | null;
	snapshot: ThematicSnapshot | null;
};

export interface ApiClient {
	getHealth(): Promise<Health>;
	registerBegin(invite?: string | null): Promise<RegistrationBegin>;
	registerFinish(body: RegistrationFinishBody): Promise<RegistrationFinishOk>;
	loginBegin(): Promise<LoginBegin>;
	loginFinish(body: LoginFinishBody): Promise<LoginOk>;
	logout(): Promise<LogoutOk>;
	getMe(): Promise<Me>;
	recover(): Promise<RecoverResponse>;
	getGraph(): Promise<GlobalTopologySnapshot>;
	getAdminLogs(limit?: number): Promise<LogsResponse>;
	getSystem(): Promise<SystemResponse>;
	mintInvite(): Promise<Invitation>;
	listInvites(): Promise<InvitationsResponse>;
	submitBraindump(verbatim: string): Promise<BraindumpDto>;
	getGraphDelta(since?: number): Promise<GraphDelta>;
	chat(query: string): Promise<ChatResponse>;
	getBraindump(id: number): Promise<Braindump>;
	getIngestStatus(id: number): Promise<IngestStatus>;
	editBraindump(id: number, verbatim: string): Promise<Braindump>;
	getMergeSuggestions(): Promise<ConceptMergeSuggestion[]>;
	approveMergeSuggestion(id: number): Promise<void>;
	rejectMergeSuggestion(id: number): Promise<void>;
	getOntology(): Promise<Ontology>;
	getOntologyProposals(): Promise<OntologyProposalsResponse>;
	approveOntologyProposal(id: number): Promise<OntologyTypeProposal>;
	rejectOntologyProposal(id: number): Promise<OntologyTypeProposal>;
	getInferenceProposals(): Promise<ChatInferenceProposal[]>;
	endorseInferenceProposal(id: number): Promise<ChatInferenceProposal>;
}

function ok(res: Response): boolean {
	return res.status >= 200 && res.status < 300;
}

// Issue #79: the backend serializes errors as `{ "error": "<message>" }`
// (backend src/error.rs). Surface that detail in the thrown Error so the UI
// can show *why* a request failed (e.g. "an invitation token is required to
// register") rather than just the status code. Non-JSON or empty bodies fall
// back to the status-only message, keeping the previous behaviour for
// opaque proxy/gateway responses.
async function readErrorDetail(res: Response): Promise<string> {
	try {
		const text = await res.text();
		if (!text) return '';
		const parsed = JSON.parse(text) as { error?: unknown };
		return typeof parsed?.error === 'string' ? parsed.error : '';
	} catch {
		return '';
	}
}

function failedError(errorLabel: string, res: Response, detail: string): Error {
	return new Error(
		`${errorLabel} failed: ${res.status}${detail ? ` - ${detail}` : ''}`
	);
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
			throw failedError(errorLabel, res, await readErrorDetail(res));
		}
		return (await res.json()) as T;
	}

	async function postJson<T>(
		path: string,
		body: unknown,
		errorLabel: string
	): Promise<T> {
		const res = await doFetch(`${baseUrl}${path}`, {
			method: 'POST',
			credentials: 'include',
			headers: {
				'content-type': 'application/json',
				accept: 'application/json'
			},
			body: JSON.stringify(body)
		});
		if (!ok(res)) {
			throw failedError(errorLabel, res, await readErrorDetail(res));
		}
		return (await res.json()) as T;
	}

	async function patchJson<T>(
		path: string,
		body: unknown,
		errorLabel: string
	): Promise<T> {
		const res = await doFetch(`${baseUrl}${path}`, {
			method: 'PATCH',
			credentials: 'include',
			headers: {
				'content-type': 'application/json',
				accept: 'application/json'
			},
			body: JSON.stringify(body)
		});
		if (!ok(res)) {
			throw failedError(errorLabel, res, await readErrorDetail(res));
		}
		return (await res.json()) as T;
	}

	async function postNoBody(path: string, errorLabel: string): Promise<void> {
		const res = await doFetch(`${baseUrl}${path}`, {
			method: 'POST',
			credentials: 'include',
			headers: { accept: 'application/json' },
			body: null
		});
		if (!ok(res)) {
			throw failedError(errorLabel, res, await readErrorDetail(res));
		}
	}

	return {
		async getHealth(): Promise<Health> {
			return getJson<Health>('/health', 'GET /health');
		},
		async registerBegin(invite?: string | null): Promise<RegistrationBegin> {
			return postJson<RegistrationBegin>(
				'/auth/register/begin',
				{ invite: invite ?? null },
				'POST /auth/register/begin'
			);
		},
		async registerFinish(
			body: RegistrationFinishBody
		): Promise<RegistrationFinishOk> {
			return postJson<RegistrationFinishOk>(
				'/auth/register/finish',
				body,
				'POST /auth/register/finish'
			);
		},
		async loginBegin(): Promise<LoginBegin> {
			return postJson<LoginBegin>(
				'/auth/login/begin',
				null,
				'POST /auth/login/begin'
			);
		},
		async loginFinish(body: LoginFinishBody): Promise<LoginOk> {
			return postJson<LoginOk>(
				'/auth/login/finish',
				body,
				'POST /auth/login/finish'
			);
		},
		async logout(): Promise<LogoutOk> {
			return postJson<LogoutOk>('/auth/logout', null, 'POST /auth/logout');
		},
		async getMe(): Promise<Me> {
			return getJson<Me>('/me', 'GET /me');
		},
		async recover(): Promise<RecoverResponse> {
			return postJson<RecoverResponse>(
				'/auth/recover',
				null,
				'POST /auth/recover'
			);
		},
		async getGraph(): Promise<GlobalTopologySnapshot> {
			return getJson<GlobalTopologySnapshot>('/graph', 'GET /graph');
		},
		async getAdminLogs(limit?: number): Promise<LogsResponse> {
			const path =
				limit !== undefined ? `/admin/logs?limit=${limit}` : '/admin/logs';
			return getJson<LogsResponse>(path, 'GET /admin/logs');
		},
		async getSystem(): Promise<SystemResponse> {
			return getJson<SystemResponse>('/admin/system', 'GET /admin/system');
		},
		async mintInvite(): Promise<Invitation> {
			return postJson<Invitation>(
				'/admin/invites',
				null,
				'POST /admin/invites'
			);
		},
		async listInvites(): Promise<InvitationsResponse> {
			return getJson<InvitationsResponse>(
				'/admin/invites',
				'GET /admin/invites'
			);
		},
		async submitBraindump(verbatim: string): Promise<BraindumpDto> {
			return postJson<BraindumpDto>(
				'/braindumps',
				{ verbatim },
				'POST /braindumps'
			);
		},
		async getGraphDelta(since?: number): Promise<GraphDelta> {
			const path =
				since !== undefined ? `/graph/delta?since=${since}` : '/graph/delta';
			return getJson<GraphDelta>(path, 'GET /graph/delta');
		},
		async chat(query: string): Promise<ChatResponse> {
			return postJson<ChatResponse>('/chat', { query }, 'POST /chat');
		},
		async getBraindump(id: number): Promise<Braindump> {
			return getJson<Braindump>(`/braindumps/${id}`, 'GET /braindumps/:id');
		},
		async getIngestStatus(id: number): Promise<IngestStatus> {
			return getJson<IngestStatus>(
				`/braindumps/${id}/ingest-status`,
				'GET /braindumps/:id/ingest-status'
			);
		},
		async editBraindump(id: number, verbatim: string): Promise<Braindump> {
			return patchJson<Braindump>(
				`/braindumps/${id}`,
				{ verbatim },
				'PATCH /braindumps/:id'
			);
		},
		async getMergeSuggestions(): Promise<ConceptMergeSuggestion[]> {
			return getJson<ConceptMergeSuggestion[]>(
				'/merge-suggestions',
				'GET /merge-suggestions'
			);
		},
		async approveMergeSuggestion(id: number): Promise<void> {
			await postNoBody(
				`/merge-suggestions/${id}/approve`,
				'POST /merge-suggestions/approve'
			);
		},
		async rejectMergeSuggestion(id: number): Promise<void> {
			await postNoBody(
				`/merge-suggestions/${id}/reject`,
				'POST /merge-suggestions/reject'
			);
		},
		async getOntology(): Promise<Ontology> {
			return getJson<Ontology>('/ontology', 'GET /ontology');
		},
		async getOntologyProposals(): Promise<OntologyProposalsResponse> {
			return getJson<OntologyProposalsResponse>(
				'/ontology/proposals',
				'GET /ontology/proposals'
			);
		},
		async approveOntologyProposal(id: number): Promise<OntologyTypeProposal> {
			return postJson<OntologyTypeProposal>(
				`/ontology/proposals/${id}/approve`,
				null,
				'POST /ontology/proposals/approve'
			);
		},
		async rejectOntologyProposal(id: number): Promise<OntologyTypeProposal> {
			return postJson<OntologyTypeProposal>(
				`/ontology/proposals/${id}/reject`,
				null,
				'POST /ontology/proposals/reject'
			);
		},
		async getInferenceProposals(): Promise<ChatInferenceProposal[]> {
			return getJson<ChatInferenceProposal[]>(
				'/chat/inferences',
				'GET /chat/inferences'
			);
		},
		async endorseInferenceProposal(id: number): Promise<ChatInferenceProposal> {
			return postJson<ChatInferenceProposal>(
				`/chat/inferences/${id}/endorse`,
				null,
				`POST /chat/inferences/${id}/endorse`
			);
		}
	};
}
