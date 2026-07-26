/** Kimi provider facade joining OAuth, headers, catalog, and chat translation. */
import { OAuthClient, refreshIfNeeded } from "./auth";
import { createChatRequest, notifyChatEvents } from "./chat-wire";
import { DEFAULT_CONFIG, type ProviderConfig } from "./config";
import { KimiHeaders } from "./headers";
import { fetchWithTimeout } from "./http";
import { DEFAULT_MODEL_ID, listModels } from "./model-catalog";
import type { ChatRequest, Credentials, Json, Model, Notify } from "./types";

/** Implements Misy protocol v2 against Kimi's subscription coding endpoints. */
export class KimiProvider {
	private readonly config: ProviderConfig;
	private readonly headers: KimiHeaders;
	private readonly oauth: OAuthClient;

	/** Creates a provider, allowing endpoint and storage overrides for local tests. */
	constructor(config: Partial<ProviderConfig> = {}) {
		this.config = { ...DEFAULT_CONFIG, ...config };
		this.headers = new KimiHeaders(this.config);
		this.oauth = new OAuthClient(this.config, this.headers);
	}

	/** Starts the device authorization flow. */
	async startAuth(method = "oauth"): Promise<Record<string, unknown>> {
		return await this.oauth.start(method);
	}

	/** Completes a device session; device flows intentionally ignore completion data. */
	async completeAuth(session: unknown): Promise<{ credentials: Credentials }> {
		return { credentials: await this.oauth.complete(session) };
	}

	/** Refreshes persisted OAuth credentials. */
	async refreshAuth(credentials: Credentials): Promise<{ credentials: Credentials }> {
		return { credentials: await this.oauth.refresh(credentials) };
	}

	/** Reports whether supplied credentials contain a usable local access token. */
	authStatus(credentials: Credentials | undefined): {
		authenticated: boolean;
		expires_at?: number;
	} {
		return this.oauth.status(credentials);
	}

	/** Performs no remote logout because Misy owns credential persistence. */
	logout(): Record<string, never> {
		return {};
	}

	/** Lists live Kimi models, falling back to the bundled catalog if the endpoint is unavailable. */
	async listModels(credentials: Credentials | undefined): Promise<Model[]> {
		return await listModels(this.config, this.headers, credentials);
	}

	/** Stops pending authorization polls during plugin shutdown. */
	cancelAuthentication(): void {
		this.oauth.cancelAll();
	}

	/** Streams one OpenAI-compatible Kimi completion, refreshing once before expiry or after 401. */
	async streamChat(
		request: ChatRequest,
		requestId: number,
		notify: Notify,
		signal: AbortSignal | undefined,
	): Promise<{ metadata: Json }> {
		try {
			let credentials = await refreshIfNeeded(this.oauth, request.credentials);
			let response = await this.chatRequest(request, credentials, signal);
			if (response.status === 401) {
				credentials = await this.oauth.refresh(credentials);
				response = await this.chatRequest(request, credentials, signal);
			}
			if (!response.ok) throw await chatError(response);
			if (!response.body) throw new Error("Kimi chat stream did not include a body");
			return { metadata: await notifyChatEvents(response.body, requestId, notify) };
		} catch (cause) {
			if (signal?.aborted) return { metadata: { completed: false, cancelled: true } };
			const message = cause instanceof Error ? cause.message : "Kimi chat request failed";
			notify("failed", { request_id: requestId, message });
			throw cause;
		}
	}

	/** Returns the preferred bundled default when present, otherwise the first returned live model. */
	defaultModel(models: readonly Model[]): string {
		const first = models[0];
		if (!first) throw new Error("Kimi model catalog is empty");
		return models.some((model) => model.id === DEFAULT_MODEL_ID) ? DEFAULT_MODEL_ID : first.id;
	}

	private async chatRequest(
		request: ChatRequest,
		credentials: Credentials,
		signal: AbortSignal | undefined,
	): Promise<Response> {
		return await fetchWithTimeout(
			endpoint(this.config.apiBaseUrl, "chat/completions"),
			{
				method: "POST",
				signal,
				headers: {
					...this.headers.common(),
					authorization: `Bearer ${credentials.access_token}`,
					accept: "text/event-stream",
					"content-type": "application/json",
				},
				body: JSON.stringify(createChatRequest(request.model_id, request.messages, request.tools)),
			},
			this.config.requestTimeoutMs,
		);
	}
}

async function chatError(response: Response): Promise<Error> {
	const text = await response.text();
	let detail = text.trim();
	try {
		const payload: unknown = JSON.parse(text);
		if (payload !== null && typeof payload === "object" && !Array.isArray(payload)) {
			const error = "error" in payload ? payload.error : payload;
			if (error !== null && typeof error === "object" && "message" in error) {
				if (typeof error.message === "string") detail = error.message;
			}
		}
	} catch {
		// Gateways may send text/plain errors; retaining the text is the useful diagnostic.
	}
	return new Error(`Kimi chat request failed (${response.status})${detail ? `: ${detail}` : ""}`);
}

function endpoint(baseUrl: string, suffix: string): URL {
	return new URL(suffix, baseUrl.endsWith("/") ? baseUrl : `${baseUrl}/`);
}
