/** Public OpenAI provider facade that joins OAuth, catalog, and Responses translation. */

import { authHeaders, fetchWithTimeout, OAuthClient, refreshIfNeeded } from "./auth";
import { DEFAULT_CONFIG, type ProviderConfig } from "./config";
import { listModels, type Model } from "./model-catalog";
import { createResponsesRequest, notifyResponseEvents } from "./responses-wire";
import type { ChatRequest, Credentials, Json, Notify } from "./types";

/** Implements Misy protocol v2 against the ChatGPT OAuth Responses backend. */
export class OpenAiProvider {
	private readonly oauth: OAuthClient;
	private readonly config: ProviderConfig;

	/** Creates a provider, allowing only endpoint values to be overridden in local tests. */
	constructor(config: Partial<ProviderConfig> = {}) {
		this.config = { ...DEFAULT_CONFIG, ...config };
		this.oauth = new OAuthClient(this.config);
	}

	/** Starts the browser OAuth flow. */
	async startAuth(method = "oauth") {
		if (method !== "oauth") throw new Error(`Unsupported OpenAI authentication method: ${method}`);
		return await this.oauth.start();
	}

	/** Exchanges a browser authorization code for opaque OAuth credentials. */
	async completeAuth(
		session: unknown,
		completion: Record<string, unknown>,
	): Promise<{ credentials: Credentials }> {
		return { credentials: await this.oauth.complete(session, completion) };
	}

	/** Refreshes persisted OAuth credentials. */
	async refreshAuth(credentials: Credentials): Promise<{ credentials: Credentials }> {
		return { credentials: await this.oauth.refresh(credentials) };
	}

	/** Reports locally-known authentication status. */
	authStatus(credentials: Credentials | undefined): {
		authenticated: boolean;
		expires_at?: number;
	} {
		return this.oauth.status(credentials);
	}

	/** Performs no remote logout because the core owns stored credentials. */
	logout(): Record<string, never> {
		return {};
	}

	/** Lists account-scoped ChatGPT Codex models, falling back to the bundled catalog on failure. */
	async listModels(credentials: Credentials): Promise<Model[]> {
		return await listModels(this.config, credentials);
	}

	/** Streams a Responses request, refreshing once before expiry or after one 401 retry. */
	async streamChat(
		request: ChatRequest,
		requestId: number,
		notify: Notify,
		signal?: AbortSignal,
	): Promise<{ metadata: Json }> {
		try {
			let credentials = await refreshIfNeeded(this.oauth, request.credentials);
			let response = await this.responsesRequest(request, credentials, signal);
			if (response.status === 401) {
				credentials = await this.oauth.refresh(credentials);
				response = await this.responsesRequest(request, credentials, signal);
			}
			if (!response.ok) throw await responsesError(response);
			if (!response.body) {
				throw new Error("OpenAI Responses stream did not include a body");
			}
			return { metadata: await notifyResponseEvents(response.body, requestId, notify) };
		} catch (cause) {
			if (signal?.aborted) {
				return { metadata: { completed: false, cancelled: true } };
			}
			const message = cause instanceof Error ? cause.message : "OpenAI Responses request failed";
			notify("failed", { request_id: requestId, message });
			throw cause;
		}
	}

	private async responsesRequest(
		request: ChatRequest,
		credentials: Credentials,
		signal: AbortSignal | undefined,
	): Promise<Response> {
		return await fetchWithTimeout(
			responsesUrl(this.config.codexBaseUrl),
			{
				method: "POST",
				signal,
				headers: {
					...authHeaders(credentials),
					accept: "text/event-stream",
					"content-type": "application/json",
				},
				body: JSON.stringify(
					createResponsesRequest(request.model_id, request.messages, request.tools),
				),
			},
			this.config.requestTimeoutMs,
		);
	}
}

async function responsesError(response: Response): Promise<Error> {
	const body = await response.text();
	const detail = errorMessage(body);
	const suffix = detail ? `: ${detail}` : "";
	return new Error(`OpenAI Responses request failed (${response.status})${suffix}`);
}

function errorMessage(body: string): string | undefined {
	try {
		const payload: unknown = JSON.parse(body);
		if (isRecord(payload)) {
			const error = isRecord(payload["error"]) ? payload["error"] : payload;
			if (typeof error["message"] === "string") return error["message"];
		}
	} catch {
		// Some gateway errors are text/plain; show that server explanation instead.
	}
	const text = body.trim();
	return text || undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}

function responsesUrl(baseUrl: string): URL {
	const url = new URL(baseUrl);
	const path = url.pathname.replace(/\/+$/, "");
	if (path.endsWith("/codex/responses")) return url;
	url.pathname = path.endsWith("/codex") ? `${path}/responses` : `${path}/codex/responses`;
	return url;
}
