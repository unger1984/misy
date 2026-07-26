/** JSON-RPC NDJSON transport and eight-method dispatcher for the Anthropic provider process. */
import { AnthropicProvider } from "./provider";
import { type Credentials, isRecord, type Json, type ToolDefinition } from "./types";

const provider = new AnthropicProvider();
const chats = new Map<number, AbortController>();

function send(value: Record<string, unknown>): void {
	process.stdout.write(`${JSON.stringify(value)}\n`);
}

async function handle(value: unknown): Promise<void> {
	if (!isRecord(value) || value["jsonrpc"] !== "2.0" || typeof value["method"] !== "string") {
		send({
			jsonrpc: "2.0",
			id: requestId(value),
			error: { code: -32600, message: "Invalid Request" },
		});
		return;
	}
	const id = value["id"];
	const method = value["method"];
	const params = record(value["params"]);
	if (method === "chat.cancel") {
		const requestId = params["request_id"];
		if (typeof requestId === "number") chats.get(requestId)?.abort();
		return;
	}
	if (id === undefined || id === null) return;
	try {
		const result = await dispatch(method, id, params);
		if (result !== undefined) send({ jsonrpc: "2.0", id, result });
	} catch (cause) {
		send({
			jsonrpc: "2.0",
			id,
			error: {
				code: -32000,
				message: cause instanceof Error ? cause.message : "Anthropic request failed",
			},
		});
	}
}

async function dispatch(
	method: string,
	id: Json,
	params: Record<string, Json>,
): Promise<unknown | undefined> {
	if (method === "auth.status") return provider.authStatus(credentials(params));
	if (method === "auth.start") return await startAuth(params);
	if (method === "auth.complete") return await completeAuth(params);
	if (method === "auth.refresh") return await provider.refreshAuth(requiredCredentials(params));
	if (method === "auth.logout") return provider.logout();
	if (method === "models.list") return await provider.listModels(requiredCredentials(params));
	if (method === "usage.get") return await provider.usage(requiredCredentials(params));
	if (method === "chat.start") {
		await chat(id, params);
		return undefined;
	}
	send({ jsonrpc: "2.0", id, error: { code: -32601, message: "Method not found" } });
	return undefined;
}

async function startAuth(params: Record<string, Json>): Promise<Json> {
	const method = params["method"];
	if (method !== undefined && typeof method !== "string") {
		throw new Error("auth.start method must be a string when provided");
	}
	const started = await provider.startAuth(typeof method === "string" ? method : "oauth");
	return { kind: "browser", url: started.url, session: started.session };
}

async function completeAuth(params: Record<string, Json>): Promise<{ credentials: Credentials }> {
	const completion = params["completion"];
	if (!isRecord(completion)) throw new Error("auth.complete completion must be an object");
	return await provider.completeAuth(params["session"], completion);
}

async function chat(id: Json, params: Record<string, Json>): Promise<void> {
	if (typeof id !== "number") throw new Error("chat.start requires a numeric JSON-RPC id");
	const modelId = params["model_id"];
	if (
		typeof modelId !== "string" ||
		!Array.isArray(params["messages"]) ||
		!Array.isArray(params["tools"]) ||
		!params["messages"].every(isRecord)
	) {
		throw new Error("chat.start requires model_id, messages, tools, and credentials");
	}
	const controller = new AbortController();
	chats.set(id, controller);
	try {
		const result = await provider.streamChat(
			{
				model_id: modelId,
				messages: params["messages"].map(record),
				tools: params["tools"].map(tool),
				credentials: requiredCredentials(params),
			},
			id,
			(method, event) => send({ jsonrpc: "2.0", method, params: event }),
			controller.signal,
		);
		send({ jsonrpc: "2.0", id, result });
	} finally {
		chats.delete(id);
	}
}

function credentials(params: Record<string, Json>): Credentials | undefined {
	const value = record(params["credentials"]);
	return typeof value["access_token"] === "string" && value["type"] === "oauth"
		? { ...value, access_token: value["access_token"], type: "oauth" }
		: undefined;
}

function requiredCredentials(params: Record<string, Json>): Credentials {
	const value = credentials(params);
	if (value === undefined) throw new Error("Anthropic OAuth credentials are required");
	return value;
}

function tool(value: Json): ToolDefinition {
	const entry = record(value);
	if (typeof entry["name"] !== "string" || typeof entry["description"] !== "string") {
		throw new Error("chat.start tools require string name and description");
	}
	return {
		name: entry["name"],
		description: entry["description"],
		input_schema: entry["input_schema"] ?? {},
	};
}

function record(value: unknown): Record<string, Json> {
	return isRecord(value) ? value : {};
}

function requestId(value: unknown): Json {
	return isRecord(value) && value["id"] !== undefined ? value["id"] : null;
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
			if (line.trim().length === 0) continue;
			const task = handleLine(line).finally(() => tasks.delete(task));
			tasks.add(task);
		}
	}
	for (const controller of chats.values()) controller.abort();
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
