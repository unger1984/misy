/**
 * JSON-RPC 2.0 NDJSON transport and protocol dispatch for Misy provider plugins.
 *
 * The transport reads one JSON object per line from stdin and reserves stdout for protocol
 * messages only. It owns the request envelope, the eight required protocol methods, `chat.cancel`
 * correlation by request id, and parameter validation; everything provider-specific lives behind
 * {@link ProviderAdapter}. Validation is strict on purpose: params cross a process boundary, so a
 * malformed request fails with a JSON-RPC error instead of a blind cast.
 */
import { requireCredentials } from "./credentials";
import { type InputFrame, NdjsonFramer } from "./ndjson";
import {
	type ChatMessage,
	type ChatRequest,
	type CredentialParser,
	isImageAttachment,
	isRecord,
	type Json,
	type Notify,
	type OAuthCredentials,
	type ProviderCredentials,
	type ToolDefinition,
} from "./types";

/** Provider-specific seams the protocol dispatch delegates to. */
export type ProviderAdapter<TCredentials extends ProviderCredentials = OAuthCredentials> = {
	/** Display name used in validation and fallback error messages. */
	readonly name: string;
	/** Error text used when a handler throws a value that is not an Error. */
	readonly requestFailureMessage: string;
	/** Narrows the provider-owned credential object before any handler sees it. */
	readonly parseCredentials: CredentialParser<TCredentials>;
	/** Reports credential presence and expiry without a network request. */
	authStatus(credentials: TCredentials | undefined): unknown;
	/** Starts the named authentication flow. */
	startAuth(method: string): Promise<unknown>;
	/** Completes an authentication attempt; `session` is the opaque `auth.start` marker. */
	completeAuth(session: unknown, completion: Record<string, Json>): Promise<unknown>;
	/** Refreshes opaque credentials on explicit core request. */
	refreshAuth(credentials: TCredentials): Promise<unknown>;
	/** Drops local authentication state; the core owns persisted credentials. */
	logout(): unknown;
	/**
	 * Lists models, optionally including a provider-local `default_model`.
	 *
	 * Credentials are optional at the protocol layer; an adapter that needs them must reject
	 * undefined itself, because some providers can serve a bundled catalog unauthenticated.
	 */
	listModels(credentials: TCredentials | undefined): Promise<unknown>;
	/** Returns the normalized usage capability version 1 report. */
	usage(credentials: TCredentials): Promise<unknown>;
	/**
	 * Streams one chat request and resolves to the JSON-RPC result.
	 *
	 * The result may carry an optional `credentials` object with rotated credentials; the
	 * transport passes it through untouched for the core to persist.
	 */
	chat(
		request: ChatRequest<TCredentials>,
		requestId: number,
		notify: Notify,
		signal: AbortSignal,
	): Promise<unknown>;
	/** Runs once after stdin closes, before pending replies are awaited. */
	shutdown?(): void;
};

/** Starts the NDJSON protocol loop for one provider adapter and runs it until stdin closes. */
export function serve<TCredentials extends ProviderCredentials>(
	adapter: ProviderAdapter<TCredentials>,
): void {
	void new ProtocolServer(adapter).run();
}

class ProtocolServer<TCredentials extends ProviderCredentials> {
	private readonly chats = new Map<number, AbortController>();

	constructor(private readonly adapter: ProviderAdapter<TCredentials>) {}

	/** Reads NDJSON frames until EOF, then aborts live chats and drains in-flight replies. */
	async run(): Promise<void> {
		const framer = new NdjsonFramer();
		const tasks = new Set<Promise<void>>();
		for await (const chunk of Bun.stdin.stream()) {
			for (const frame of framer.push(chunk)) this.schedule(frame, tasks);
		}
		const finalFrame = framer.finish();
		if (finalFrame !== undefined) this.schedule(finalFrame, tasks);
		for (const controller of this.chats.values()) controller.abort();
		this.adapter.shutdown?.();
		await Promise.allSettled(tasks);
	}

	private schedule(frame: InputFrame, tasks: Set<Promise<void>>): void {
		if (frame.kind === "too_large") {
			this.send({
				jsonrpc: "2.0",
				id: null,
				error: { code: -32700, message: "Input frame exceeds 32 MiB limit" },
			});
			return;
		}
		if (frame.line.trim().length === 0) return;
		const task = this.handleLine(frame.line).finally(() => tasks.delete(task));
		tasks.add(task);
	}

	private async handleLine(line: string): Promise<void> {
		try {
			await this.handle(JSON.parse(line) as unknown);
		} catch {
			this.send({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } });
		}
	}

	private send(value: Record<string, unknown>): void {
		process.stdout.write(`${JSON.stringify(value)}\n`);
	}

	private async handle(value: unknown): Promise<void> {
		if (!isRecord(value) || value["jsonrpc"] !== "2.0" || typeof value["method"] !== "string") {
			this.send({
				jsonrpc: "2.0",
				id: requestId(value),
				error: { code: -32600, message: "Invalid Request" },
			});
			return;
		}
		const id = value["id"];
		const params = recordOf(value["params"]);
		if (value["method"] === "chat.cancel") {
			const cancelId = params["request_id"];
			if (typeof cancelId === "number") this.chats.get(cancelId)?.abort();
			return;
		}
		if (id === undefined || id === null) return;
		try {
			const result = await this.dispatch(value["method"], id, params);
			if (result !== undefined) this.send({ jsonrpc: "2.0", id, result });
		} catch (cause) {
			this.send({
				jsonrpc: "2.0",
				id,
				error: {
					code: -32000,
					message: cause instanceof Error ? cause.message : this.adapter.requestFailureMessage,
				},
			});
		}
	}

	private async dispatch(method: string, id: Json, params: Record<string, Json>): Promise<unknown> {
		switch (method) {
			case "auth.status":
				return this.adapter.authStatus(this.parseCredentials(params));
			case "auth.start":
				return await this.adapter.startAuth(authMethod(params));
			case "auth.complete":
				return await this.adapter.completeAuth(params["session"], completion(params));
			case "auth.refresh":
				return await this.adapter.refreshAuth(this.requiredCredentials(params));
			case "auth.logout":
				return this.adapter.logout();
			case "models.list":
				return await this.adapter.listModels(this.parseCredentials(params));
			case "usage.get":
				return await this.adapter.usage(this.requiredCredentials(params));
			case "chat.start":
				return await this.chat(id, params);
			default:
				this.send({ jsonrpc: "2.0", id, error: { code: -32601, message: "Method not found" } });
				return undefined;
		}
	}

	private parseCredentials(params: Record<string, Json>): TCredentials | undefined {
		return this.adapter.parseCredentials(params["credentials"]);
	}

	private requiredCredentials(params: Record<string, Json>): TCredentials {
		return requireCredentials(this.parseCredentials(params), this.adapter.name);
	}

	private async chat(id: Json, params: Record<string, Json>): Promise<undefined> {
		if (typeof id !== "number") throw new Error("chat.start requires a numeric JSON-RPC id");
		const request = chatRequest(params, this.requiredCredentials(params));
		const controller = new AbortController();
		this.chats.set(id, controller);
		try {
			const notify: Notify = (method, event) =>
				this.send({ jsonrpc: "2.0", method, params: event });
			const result = await this.adapter.chat(request, id, notify, controller.signal);
			if (result !== undefined) this.send({ jsonrpc: "2.0", id, result });
			return undefined;
		} finally {
			this.chats.delete(id);
		}
	}
}

function chatRequest<TCredentials extends ProviderCredentials>(
	params: Record<string, Json>,
	credentials: TCredentials,
): ChatRequest<TCredentials> {
	const modelId = params["model_id"];
	if (typeof modelId !== "string" || modelId.trim().length === 0) {
		throw new Error("chat.start requires a non-empty model_id");
	}
	const messages = params["messages"];
	if (!Array.isArray(messages) || !messages.every(isRecord)) {
		throw new Error("chat.start requires messages to be an array of objects");
	}
	const tools = params["tools"];
	if (!Array.isArray(tools)) throw new Error("chat.start requires tools to be an array");
	const thinking = params["thinking"];
	if (thinking !== undefined && (typeof thinking !== "string" || thinking.length === 0)) {
		throw new Error("chat.start thinking must be a non-empty string when provided");
	}
	const maxOutputTokens = params["max_output_tokens"];
	if (
		maxOutputTokens !== undefined &&
		(typeof maxOutputTokens !== "number" ||
			!Number.isSafeInteger(maxOutputTokens) ||
			maxOutputTokens <= 0)
	) {
		throw new Error("chat.start max_output_tokens must be a positive integer when provided");
	}
	return {
		model_id: modelId,
		...(typeof thinking === "string" ? { thinking } : {}),
		...(typeof maxOutputTokens === "number" ? { max_output_tokens: maxOutputTokens } : {}),
		messages: messages.map(message),
		tools: tools.map(toolDefinition),
		credentials,
	};
}

function message(value: Json): ChatMessage {
	const entry = recordOf(value);
	validateAttachments(entry["attachments"]);
	const results = entry["tool_results"];
	if (results !== undefined) {
		if (!Array.isArray(results) || !results.every(isRecord)) {
			throw new Error("chat.start tool_results must be an array of objects");
		}
		for (const result of results) validateAttachments(recordOf(result)["attachments"]);
	}
	return entry;
}

function validateAttachments(value: Json | undefined): void {
	if (value === undefined) return;
	if (!Array.isArray(value) || !value.every(isImageAttachment)) {
		throw new Error("chat.start attachments require base64 image/png objects");
	}
}

function toolDefinition(value: Json): ToolDefinition {
	if (
		!isRecord(value) ||
		typeof value["name"] !== "string" ||
		typeof value["description"] !== "string"
	) {
		throw new Error("chat.start tools require string name and description");
	}
	return {
		name: value["name"],
		description: value["description"],
		input_schema: value["input_schema"] ?? {},
	};
}

function authMethod(params: Record<string, Json>): string {
	const method = params["method"];
	if (method !== undefined && typeof method !== "string") {
		throw new Error("auth.start method must be a string when provided");
	}
	return typeof method === "string" ? method : "oauth";
}

function completion(params: Record<string, Json>): Record<string, Json> {
	const value = params["completion"];
	if (value !== undefined && !isRecord(value)) {
		throw new Error("auth.complete completion must be an object");
	}
	return isRecord(value) ? value : {};
}

function recordOf(value: unknown): Record<string, Json> {
	return isRecord(value) ? value : {};
}

function requestId(value: unknown): Json {
	return isRecord(value) && value["id"] !== undefined ? value["id"] : null;
}
