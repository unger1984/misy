/**
 * AnyModel HTTP facade for prompt authentication, catalog validation, and chat streaming.
 */
import { endpointUrl, fetchWithTimeout } from "@misy/provider-sdk";
import { authStatus, completeAuth, startAuth } from "./auth";
import { createChatRequest, notifyChatEvents, safeMessage } from "./chat-wire";
import { DEFAULT_CONFIG, type ProviderConfig } from "./config";
import { listModelCatalog } from "./model-catalog";
import type { ApiKeyCredentials, ChatRequest, Json, Model, Notify } from "./types";

/** Implements Misy protocol v2 against AnyModel's OpenAI-compatible endpoints. */
export class AnyModelProvider {
	private readonly config: ProviderConfig;

	/** Creates a provider with production defaults or injected local-test endpoints. */
	constructor(config: Partial<ProviderConfig> = {}) {
		this.config = { ...DEFAULT_CONFIG, ...config };
	}

	/** Returns the protocol prompt descriptor for AnyModel API-key authorization. */
	startAuth(method = "api_key"): Record<string, Json> {
		return startAuth(method);
	}

	/** Validates a submitted key against the catalog before allowing the core to persist it. */
	async completeAuth(
		session: unknown,
		completion: Record<string, Json>,
	): Promise<{ credentials: ApiKeyCredentials }> {
		const credentials = completeAuth(session, completion);
		await listModelCatalog(this.config, credentials);
		return { credentials };
	}

	/** Refreshes an API key by revalidating it against the authenticated catalog. */
	async refreshAuth(credentials: ApiKeyCredentials): Promise<{ credentials: ApiKeyCredentials }> {
		await listModelCatalog(this.config, credentials);
		return { credentials };
	}

	/** Performs no remote call; the core owns persistence and this checks local key shape only. */
	authStatus(credentials: ApiKeyCredentials | undefined): { authenticated: boolean } {
		return authStatus(credentials);
	}

	/** Does not revoke remotely because core-owned credentials are removed after this result. */
	logout(): Record<string, never> {
		return {};
	}

	/** Lists the live authenticated catalog without a bundled fallback. */
	async listModels(credentials: ApiKeyCredentials | undefined): Promise<Model[]> {
		if (credentials === undefined) throw new Error("AnyModel API key credentials are required");
		return await listModelCatalog(this.config, credentials);
	}

	/** Uses the first validated upstream entry as the provider-local default model. */
	defaultModel(models: readonly Model[]): string {
		const first = models[0];
		if (first === undefined) throw new Error("AnyModel model catalog is empty");
		return first.id;
	}

	/** Exists for protocol completeness; the manifest intentionally does not negotiate usage. */
	usage(): never {
		throw new Error("AnyModel usage is not supported");
	}

	/** Streams an exact provider-local model ID, never retrying a rejected credential. */
	async streamChat(
		request: ChatRequest<ApiKeyCredentials>,
		requestId: number,
		notify: Notify,
		signal: AbortSignal | undefined,
	): Promise<{ metadata: Json }> {
		try {
			const response = await fetchWithTimeout(
				endpointUrl(this.config.baseUrl, "chat/completions"),
				{
					method: "POST",
					signal,
					headers: {
						accept: "text/event-stream",
						authorization: `Bearer ${request.credentials.api_key}`,
						"content-type": "application/json",
					},
					body: JSON.stringify(
						createChatRequest(
							request.model_id,
							request.messages,
							request.tools,
							request.max_output_tokens,
							request.thinking,
						),
					),
				},
				this.config.requestTimeoutMs,
			);
			if (!response.ok) throw chatResponseError(response);
			if (!response.body) throw new Error("AnyModel chat stream did not include a body");
			return {
				metadata: await notifyChatEvents(
					response.body,
					requestId,
					notify,
					request.credentials.api_key,
				),
			};
		} catch (cause) {
			if (signal?.aborted) return { metadata: { completed: false, cancelled: true } };
			const message =
				cause instanceof Error
					? safeMessage(cause.message, request.credentials.api_key)
					: "AnyModel chat request failed";
			notify("failed", { request_id: requestId, message });
			// The terminal notification is the stream's authoritative failure. Returning a normal
			// response keeps the SDK from racing it with a second JSON-RPC error for the same request.
			return { metadata: { completed: false, failed: true } };
		}
	}
}

function chatResponseError(response: Response): Error {
	if (response.status === 429) {
		return new Error("AnyModel rate limit exceeded (429); retry later or select another model");
	}
	return new Error(`AnyModel chat request failed (${response.status})`);
}
