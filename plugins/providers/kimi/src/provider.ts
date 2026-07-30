/** Kimi provider facade joining OAuth, headers, catalog, and chat translation. */

import { endpointUrl, fetchWithTimeout, preferredDefaultModel } from "@misy/provider-sdk";
import { createAnthropicRequest, notifyAnthropicEvents } from "./anthropic-wire";
import { OAuthClient, refreshIfNeeded } from "./auth";
import { createChatRequest, notifyChatEvents } from "./chat-wire";
import { DEFAULT_CONFIG, type ProviderConfig } from "./config";
import { KimiHeaders } from "./headers";
import {
	DEFAULT_MODEL_ID,
	discoverModelProtocol,
	type KimiProtocol,
	listModelCatalog,
	ModelCatalogRequestError,
} from "./model-catalog";
import type { ChatRequest, Credentials, Json, Model, Notify } from "./types";
import { fetchUsage, type UsageReport, UsageRequestError } from "./usage";

/** Implements Misy protocol v2 against Kimi's subscription coding endpoints. */
export class KimiProvider {
	private readonly config: ProviderConfig;
	private readonly headers: KimiHeaders;
	private readonly oauth: OAuthClient;
	private protocols: ReadonlyMap<string, KimiProtocol>;
	private modelSource: "remote" | "bundled" = "bundled";

	/** Creates a provider, allowing endpoint and storage overrides for local tests. */
	constructor(config: Partial<ProviderConfig> = {}) {
		this.config = { ...DEFAULT_CONFIG, ...config };
		this.headers = new KimiHeaders(this.config);
		this.oauth = new OAuthClient(this.config, this.headers);
		this.protocols = new Map();
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
		const catalog = await listModelCatalog(this.config, this.headers, credentials);
		this.modelSource = catalog.authoritative ? "remote" : "bundled";
		if (catalog.authoritative) this.protocols = catalog.protocols;
		return catalog.models;
	}

	/** Reports whether the latest catalog came from live discovery or bundled fallback. */
	catalogSource(): "remote" | "bundled" {
		return this.modelSource;
	}

	/** Returns normalized Kimi Coding subscription limits plus credentials rotated by a refresh. */
	async usage(
		credentials: Credentials,
		signal?: AbortSignal,
	): Promise<UsageReport & { credentials?: Credentials }> {
		let current = await refreshIfNeeded(this.oauth, credentials);
		let report: UsageReport;
		try {
			report = await fetchUsage(this.config, this.headers, current, signal);
		} catch (cause) {
			if (!(cause instanceof UsageRequestError) || cause.status !== 401) throw cause;
			current = await this.oauth.refresh(current);
			report = await fetchUsage(this.config, this.headers, current, signal);
		}
		// Token rotation invalidates the stored refresh token, so refreshed credentials
		// must travel back to the core instead of being dropped with the request.
		return current === credentials ? report : { ...report, credentials: current };
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
	): Promise<{ metadata: Json; credentials?: Credentials }> {
		try {
			let credentials = await refreshIfNeeded(this.oauth, request.credentials);
			let protocol: KimiProtocol;
			try {
				protocol = await this.resolveProtocol(request.model_id, credentials, signal);
			} catch (cause) {
				if (!(cause instanceof ModelCatalogRequestError) || cause.status !== 401) throw cause;
				credentials = await this.oauth.refresh(credentials);
				protocol = await this.resolveProtocol(request.model_id, credentials, signal);
			}
			let response = await this.chatRequest(request, credentials, protocol, signal);
			if (response.status === 401) {
				credentials = await this.oauth.refresh(credentials);
				response = await this.chatRequest(request, credentials, protocol, signal);
			}
			if (!response.ok) throw await chatError(response);
			if (!response.body) throw new Error("Kimi chat stream did not include a body");
			const metadata =
				protocol === "anthropic"
					? await notifyAnthropicEvents(response.body, requestId, notify)
					: await notifyChatEvents(response.body, requestId, notify);
			// Same rotation contract as usage(): the core persists replacement credentials.
			return credentials === request.credentials ? { metadata } : { metadata, credentials };
		} catch (cause) {
			if (signal?.aborted) return { metadata: { completed: false, cancelled: true } };
			const message = cause instanceof Error ? cause.message : "Kimi chat request failed";
			notify("failed", { request_id: requestId, message });
			throw cause;
		}
	}

	private async resolveProtocol(
		modelId: string,
		credentials: Credentials,
		signal: AbortSignal | undefined,
	): Promise<KimiProtocol> {
		const known = this.protocols.get(modelId);
		if (known !== undefined) return known;
		const discovered = await discoverModelProtocol(
			this.config,
			this.headers,
			credentials,
			modelId,
			signal,
		);
		if (discovered !== undefined) {
			this.protocols = new Map(this.protocols).set(modelId, discovered);
			return discovered;
		}
		throw new Error(`Kimi model protocol is unavailable for ${modelId}; refresh the model catalog`);
	}

	/** Returns the preferred bundled default when present, otherwise the first returned live model. */
	defaultModel(models: readonly Model[]): string {
		const selected = preferredDefaultModel(models, DEFAULT_MODEL_ID);
		if (selected === undefined) throw new Error("Kimi model catalog is empty");
		return selected;
	}

	private async chatRequest(
		request: ChatRequest,
		credentials: Credentials,
		protocol: KimiProtocol,
		signal: AbortSignal | undefined,
	): Promise<Response> {
		return await fetchWithTimeout(
			endpointUrl(
				this.config.apiBaseUrl,
				protocol === "anthropic" ? "messages" : "chat/completions",
			),
			{
				method: "POST",
				signal,
				headers: {
					...this.headers.common(),
					authorization: `Bearer ${credentials.access_token}`,
					accept: "text/event-stream",
					"content-type": "application/json",
					...(protocol === "anthropic" ? { "anthropic-version": "2023-06-01" } : {}),
				},
				body: JSON.stringify(
					protocol === "anthropic"
						? createAnthropicRequest(
								request.model_id,
								request.messages,
								request.tools,
								request.max_output_tokens,
							)
						: createChatRequest(
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
