/** Public Anthropic provider facade joining OAuth, model discovery, and Messages streaming. */
import { fetchWithTimeout, OAuthClient, refreshIfNeeded } from "./auth";
import { DEFAULT_CONFIG, type ProviderConfig } from "./config";
import { createMessagesRequest, notifyMessageEvents } from "./messages-wire";
import { defaultModel, listModels, type Model } from "./model-catalog";
import type { ChatRequest, Credentials, Json, Notify } from "./types";

const CLAUDE_CODE_BETAS =
	"claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14," +
	"context-management-2025-06-27,prompt-caching-scope-2026-01-05," +
	"mid-conversation-system-2026-04-07,advanced-tool-use-2025-11-20," +
	"effort-2025-11-24,extended-cache-ttl-2025-04-11";

/** Implements Misy protocol v2 against Anthropic's subscription Messages endpoint. */
export class AnthropicProvider {
	private readonly config: ProviderConfig;
	private readonly oauth: OAuthClient;

	/** Creates a provider, allowing endpoint overrides only for local fake-server tests. */
	constructor(config: Partial<ProviderConfig> = {}) {
		this.config = { ...DEFAULT_CONFIG, ...config };
		this.oauth = new OAuthClient(this.config);
	}

	/** Starts browser OAuth for the only supported Anthropic authentication method. */
	async startAuth(method = "oauth") {
		if (method !== "oauth")
			throw new Error(`Unsupported Anthropic authentication method: ${method}`);
		return await this.oauth.start();
	}

	/** Completes browser OAuth and returns opaque credentials for the core to persist. */
	async completeAuth(
		session: unknown,
		completion: Record<string, unknown>,
	): Promise<{ credentials: Credentials }> {
		return { credentials: await this.oauth.complete(session, completion) };
	}

	/** Refreshes opaque credentials on explicit core request. */
	async refreshAuth(credentials: Credentials): Promise<{ credentials: Credentials }> {
		return { credentials: await this.oauth.refresh(credentials) };
	}

	/** Reports credential presence and expiry without a network request. */
	authStatus(credentials: Credentials | undefined): {
		authenticated: boolean;
		expires_at?: number;
	} {
		return this.oauth.status(credentials);
	}

	/** Does not revoke remotely because the core owns persisted credentials. */
	logout(): Record<string, never> {
		return {};
	}

	/** Lists dynamically available models, falling back to the package catalog when unavailable. */
	async listModels(credentials: Credentials): Promise<{ models: Model[]; default_model?: string }> {
		const models = await listModels(this.config, credentials);
		const selected = defaultModel(models);
		return selected === undefined ? { models } : { models, default_model: selected };
	}

	/** Streams Messages events, refreshing before expiry and retrying exactly once after a 401. */
	async streamChat(
		request: ChatRequest,
		requestId: number,
		notify: Notify,
		signal?: AbortSignal,
	): Promise<{ metadata: Json }> {
		try {
			let credentials = await refreshIfNeeded(this.oauth, request.credentials);
			let response = await this.messagesRequest(request, credentials, signal);
			if (response.status === 401) {
				credentials = await this.oauth.refresh(credentials);
				response = await this.messagesRequest(request, credentials, signal);
			}
			if (!response.ok) throw await messagesError(response);
			if (response.body === null)
				throw new Error("Anthropic Messages stream did not include a body");
			return { metadata: await notifyMessageEvents(response.body, requestId, notify) };
		} catch (cause) {
			if (signal?.aborted === true) return { metadata: { completed: false, cancelled: true } };
			const message = cause instanceof Error ? cause.message : "Anthropic Messages request failed";
			notify("failed", { request_id: requestId, message });
			throw cause;
		}
	}

	private async messagesRequest(
		request: ChatRequest,
		credentials: Credentials,
		signal: AbortSignal | undefined,
	): Promise<Response> {
		return await fetchWithTimeout(
			endpoint(this.config.apiBaseUrl, "v1/messages"),
			{
				method: "POST",
				signal,
				headers: {
					authorization: `Bearer ${credentials["access_token"]}`,
					"anthropic-version": "2023-06-01",
					"anthropic-beta": CLAUDE_CODE_BETAS,
					"anthropic-dangerous-direct-browser-access": "true",
					"content-type": "application/json",
					"user-agent": "claude-cli/2.1.165",
					"x-app": "cli",
				},
				body: JSON.stringify(
					createMessagesRequest(request.model_id, request.messages, request.tools),
				),
			},
			this.config.requestTimeoutMs,
		);
	}
}

async function messagesError(response: Response): Promise<Error> {
	const body = await response.text();
	const detail = errorDetail(body);
	return new Error(`Anthropic Messages request failed (${response.status})${detail}`);
}

function errorDetail(body: string): string {
	try {
		const value: unknown = JSON.parse(body);
		if (isUnknownRecord(value)) {
			const error = value["error"];
			if (isUnknownRecord(error)) {
				if (typeof error["message"] === "string") return `: ${error["message"]}`;
			}
			if (typeof value["message"] === "string") return `: ${value["message"]}`;
		}
	} catch {
		// Gateways can return text errors; preserve the concise body for diagnosis.
	}
	const text = body.trim();
	return text.length > 0 ? `: ${text}` : "";
}

function isUnknownRecord(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}

function endpoint(baseUrl: string, path: string): URL {
	return new URL(path, baseUrl.endsWith("/") ? baseUrl : `${baseUrl}/`);
}
