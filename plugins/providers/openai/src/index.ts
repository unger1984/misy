/** JSON-RPC NDJSON transport for the standalone OpenAI provider. */
import { DEFAULT_MODEL_ID } from "./model-catalog";
import { OpenAiProvider } from "./provider";
import type { Credentials, Json } from "./types";

const provider = new OpenAiProvider();
const chats = new Map<number, AbortController>();
function send(value: Record<string, unknown>): void {
	process.stdout.write(`${JSON.stringify(value)}\n`);
}
function record(value: unknown): Record<string, unknown> {
	return value !== null && typeof value === "object" && !Array.isArray(value)
		? (value as Record<string, unknown>)
		: {};
}
function credentials(params: Record<string, unknown>): Credentials | undefined {
	const value = record(params["credentials"]);
	return typeof value["access_token"] === "string" && value["type"] === "oauth"
		? (value as Credentials)
		: undefined;
}
async function handle(value: unknown): Promise<void> {
	const request = record(value);
	const id = request["id"];
	const method = request["method"];
	const params = record(request["params"]);
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
		if (method === "auth.status")
			send({ jsonrpc: "2.0", id, result: provider.authStatus(credentials(params)) });
		else if (method === "auth.start") {
			const authMethod = typeof params["method"] === "string" ? params["method"] : "oauth";
			const start = await provider.startAuth(authMethod);
			send({ jsonrpc: "2.0", id, result: { url: start.url, session: start.session } });
		} else if (method === "auth.complete")
			send({
				jsonrpc: "2.0",
				id,
				result: await provider.completeAuth(params["completion"], record(params["completion"])),
			});
		else if (method === "auth.refresh")
			send({
				jsonrpc: "2.0",
				id,
				result: await provider.refreshAuth(required(credentials(params))),
			});
		else if (method === "auth.logout") send({ jsonrpc: "2.0", id, result: provider.logout() });
		else if (method === "models.list")
			send({
				jsonrpc: "2.0",
				id,
				result: { models: provider.listModels(), default_model: DEFAULT_MODEL_ID },
			});
		else if (method === "chat.start") await chat(id, params);
		else send({ jsonrpc: "2.0", id, error: { code: -32601, message: "Method not found" } });
	} catch (cause) {
		send({
			jsonrpc: "2.0",
			id,
			error: {
				code: -32000,
				message: cause instanceof Error ? cause.message : "OpenAI provider request failed",
			},
		});
	}
}
async function chat(id: unknown, params: Record<string, unknown>): Promise<void> {
	if (
		typeof id !== "number" ||
		typeof params["model_id"] !== "string" ||
		!Array.isArray(params["messages"]) ||
		!Array.isArray(params["tools"])
	)
		throw new Error("chat.start requires numeric id, model_id, messages, tools, and credentials");
	const controller = new AbortController();
	chats.set(id, controller);
	try {
		const result = await provider.streamChat(
			{
				model_id: params["model_id"],
				messages: params["messages"].map(record),
				tools: params["tools"] as never[],
				credentials: required(credentials(params)),
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
function required(value: Credentials | undefined): Credentials {
	if (!value) throw new Error("OAuth credentials are required");
	return value;
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
	await Promise.allSettled(tasks);
}
async function handleLine(line: string): Promise<void> {
	try {
		await handle(JSON.parse(line) as Json);
	} catch {
		send({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } });
	}
}
void main();
