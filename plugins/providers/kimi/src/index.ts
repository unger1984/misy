/** JSON-RPC NDJSON transport and dispatch for the standalone Kimi provider process. */
import { KimiProvider } from "./provider";
import {
	type ChatRequest,
	type Credentials,
	isRecord,
	type Json,
	type ToolDefinition,
} from "./types";

const provider = new KimiProvider();
const chats = new Map<number, AbortController>();

function send(value: Record<string, unknown>): void {
	process.stdout.write(`${JSON.stringify(value)}\n`);
}

async function handle(value: unknown): Promise<void> {
	const request = isRecord(value) ? value : {};
	const id = request["id"];
	const method = request["method"];
	const params = isRecord(request["params"]) ? request["params"] : {};
	if (request["jsonrpc"] !== "2.0" || typeof method !== "string") {
		send({ jsonrpc: "2.0", id: id ?? null, error: { code: -32600, message: "Invalid Request" } });
		return;
	}
	if (method === "chat.cancel") {
		if (typeof params["request_id"] === "number") chats.get(params["request_id"])?.abort();
		return;
	}
	if (id === undefined || id === null) return;
	try {
		switch (method) {
			case "auth.status":
				send({ jsonrpc: "2.0", id, result: provider.authStatus(credentials(params)) });
				return;
			case "auth.start": {
				const authMethod = typeof params["method"] === "string" ? params["method"] : "oauth";
				send({ jsonrpc: "2.0", id, result: await provider.startAuth(authMethod) });
				return;
			}
			case "auth.complete":
				send({ jsonrpc: "2.0", id, result: await provider.completeAuth(params["session"]) });
				return;
			case "auth.refresh":
				send({
					jsonrpc: "2.0",
					id,
					result: await provider.refreshAuth(required(credentials(params))),
				});
				return;
			case "auth.logout":
				send({ jsonrpc: "2.0", id, result: provider.logout() });
				return;
			case "models.list": {
				const models = await provider.listModels(credentials(params));
				send({
					jsonrpc: "2.0",
					id,
					result: {
						models,
						default_model: provider.defaultModel(models),
					},
				});
				return;
			}
			case "chat.start":
				await chat(id, params);
				return;
			default:
				send({ jsonrpc: "2.0", id, error: { code: -32601, message: "Method not found" } });
		}
	} catch (cause) {
		send({
			jsonrpc: "2.0",
			id,
			error: {
				code: -32000,
				message: cause instanceof Error ? cause.message : "Kimi provider request failed",
			},
		});
	}
}

async function chat(id: unknown, params: Record<string, unknown>): Promise<void> {
	if (typeof id !== "number") throw new Error("chat.start requires a numeric request id");
	const request = parseChatRequest(params);
	const controller = new AbortController();
	chats.set(id, controller);
	try {
		const result = await provider.streamChat(
			request,
			id,
			(method, event) => send({ jsonrpc: "2.0", method, params: event }),
			controller.signal,
		);
		send({ jsonrpc: "2.0", id, result });
	} finally {
		chats.delete(id);
	}
}

function parseChatRequest(params: Record<string, unknown>): ChatRequest {
	if (typeof params["model_id"] !== "string" || !params["model_id"].trim()) {
		throw new Error("chat.start requires a non-empty model_id");
	}
	if (!Array.isArray(params["messages"]) || !params["messages"].every(isRecord)) {
		throw new Error("chat.start requires messages to be an array of objects");
	}
	if (!Array.isArray(params["tools"])) throw new Error("chat.start requires tools to be an array");
	return {
		model_id: params["model_id"],
		messages: params["messages"],
		tools: params["tools"].map(parseTool),
		credentials: required(credentials(params)),
	};
}

function parseTool(value: unknown): ToolDefinition {
	if (
		!isRecord(value) ||
		typeof value["name"] !== "string" ||
		typeof value["description"] !== "string"
	) {
		throw new Error("chat.start tools require name and description strings");
	}
	const schema = jsonValue(value["input_schema"]);
	if (schema === undefined) throw new Error("chat.start tool input_schema must be JSON");
	return { name: value["name"], description: value["description"], input_schema: schema };
}

function credentials(params: Record<string, unknown>): Credentials | undefined {
	const value = params["credentials"];
	if (!isRecord(value) || typeof value["access_token"] !== "string" || value["type"] !== "oauth") {
		return undefined;
	}
	const parsed = jsonValue(value);
	return parsed !== undefined && isRecord(parsed)
		? { ...parsed, access_token: value["access_token"], type: "oauth" }
		: undefined;
}

function required(value: Credentials | undefined): Credentials {
	if (!value) throw new Error("Kimi OAuth credentials are required");
	return value;
}

function jsonValue(value: unknown): Json | undefined {
	if (
		value === null ||
		typeof value === "string" ||
		typeof value === "number" ||
		typeof value === "boolean"
	) {
		return value;
	}
	if (Array.isArray(value)) {
		const parsed = value.map(jsonValue);
		return parsed.every((item) => item !== undefined) ? parsed : undefined;
	}
	if (!isRecord(value)) return undefined;
	const parsed: { [key: string]: Json } = {};
	for (const [key, item] of Object.entries(value)) {
		const converted = jsonValue(item);
		if (converted === undefined) return undefined;
		parsed[key] = converted;
	}
	return parsed;
}

async function main(): Promise<void> {
	const decoder = new TextDecoder();
	let pending = "";
	const tasks = new Set<Promise<void>>();
	for await (const chunk of Bun.stdin.stream()) {
		pending += decoder.decode(chunk, { stream: true });
		const lines = pending.split("\n");
		pending = lines.pop() ?? "";
		for (const line of lines) {
			if (!line.trim()) continue;
			const task = handleLine(line).finally(() => tasks.delete(task));
			tasks.add(task);
		}
	}
	for (const controller of chats.values()) controller.abort();
	provider.cancelAuthentication();
	await Promise.allSettled(tasks);
}

async function handleLine(line: string): Promise<void> {
	try {
		await handle(JSON.parse(line) as unknown);
	} catch {
		send({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } });
	}
}

void main();
