import { afterEach, expect, test } from "bun:test";
import { createServer } from "node:net";
import { postOAuthJson } from "../src/oauth-http";
import { AnthropicProvider } from "../src/provider";
import type { Credentials } from "../src/types";

type CapturedRequest = {
	path: string;
	headers: Headers;
	body: unknown;
	signal: AbortSignal;
};

const servers: Array<ReturnType<typeof Bun.serve>> = [];
const rawServers: Array<ReturnType<typeof createServer>> = [];

afterEach(async () => {
	while (servers.length > 0) servers.pop()?.stop(true);
	while (rawServers.length > 0) {
		const server = rawServers.pop();
		if (server !== undefined) await closeServer(server);
	}
});

function fakeServer(handler: (request: CapturedRequest) => Response | Promise<Response>): string {
	const server = Bun.serve({
		hostname: "127.0.0.1",
		port: 0,
		fetch: async (request) => {
			const raw = request.method === "POST" ? await request.text() : "";
			return await handler({
				path: new URL(request.url).pathname,
				headers: request.headers,
				body: raw.length > 0 ? JSON.parse(raw) : undefined,
				signal: request.signal,
			});
		},
	});
	servers.push(server);
	return `http://127.0.0.1:${server.port}`;
}

function credentials(overrides: Partial<Credentials> = {}): Credentials {
	return { access_token: "access", refresh_token: "refresh", type: "oauth", ...overrides };
}

async function rawServer(response: string): Promise<string> {
	const server = createServer((socket) => {
		socket.once("data", () => socket.end(response));
	});
	await new Promise<void>((resolve, reject) => {
		server.once("error", reject);
		server.listen(0, "127.0.0.1", resolve);
	});
	rawServers.push(server);
	const address = server.address();
	if (address === null || typeof address === "string")
		throw new Error("Raw test server did not expose a port");
	return `http://127.0.0.1:${address.port}`;
}

async function closeServer(server: ReturnType<typeof createServer>): Promise<void> {
	await new Promise<void>((resolve, reject) => {
		server.close((error) => (error === undefined ? resolve() : reject(error)));
	});
}

test("rejects a truncated raw OAuth response without throwing from a socket callback", async () => {
	const base = await rawServer("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nabc");
	await expect(
		postOAuthJson(
			new URL("/v1/oauth/token", base),
			{ "content-type": "application/json" },
			"{}",
			100,
		),
	).rejects.toThrow("ended a chunk early");
});

test("exchanges a browser callback code without Accept and supports code#state", async () => {
	let exchange: CapturedRequest | undefined;
	const base = fakeServer((request) => {
		exchange = request;
		return Response.json({ access_token: "access", refresh_token: "refresh", expires_in: 600 });
	});
	const provider = new AnthropicProvider({ apiBaseUrl: base, authorizeUrl: `${base}/authorize` });
	const started = await provider.startAuth();
	const authorization = new URL(started.url);
	expect(authorization.origin).toBe(base);
	expect(authorization.searchParams.get("redirect_uri")).toBe("http://localhost:54545/callback");
	expect(authorization.searchParams.get("code_challenge_method")).toBe("S256");
	const callback = new URL("http://localhost:54545/callback");
	callback.searchParams.set("code", "callback-code#replacement-state");
	callback.searchParams.set("state", authorization.searchParams.get("state") ?? "");
	await fetch(callback);
	const completed = await provider.completeAuth(started.session, {});
	expect(exchange?.path).toBe("/v1/oauth/token");
	expect(exchange?.headers.has("accept")).toBeFalse();
	expect(exchange?.body).toMatchObject({
		grant_type: "authorization_code",
		client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
		code: "callback-code",
		state: "replacement-state",
		redirect_uri: "http://localhost:54545/callback",
	});
	expect(completed.credentials).toMatchObject({ access_token: "access", type: "oauth" });
});

test("releases an abandoned callback listener and reports an occupied callback port", async () => {
	const base = fakeServer(() => Response.json({ access_token: "access" }));
	const provider = new AnthropicProvider({ apiBaseUrl: base, authTimeoutMs: 10 });
	await provider.startAuth();
	await Bun.sleep(30);
	const restarted = await provider.startAuth();
	await Bun.sleep(30);
	expect(restarted.session.id).toBeString();
	const occupied = Bun.serve({
		hostname: "127.0.0.1",
		port: 54545,
		fetch: () => new Response(),
	});
	servers.push(occupied);
	await expect(provider.startAuth()).rejects.toThrow("callback port 54545 is already in use");
});

test("refreshes with the Anthropic OAuth fingerprint and explains invalid_grant", async () => {
	const seen: CapturedRequest[] = [];
	let invalidGrant = false;
	const base = fakeServer((request) => {
		seen.push(request);
		if (invalidGrant) return Response.json({ error: "invalid_grant" }, { status: 400 });
		return Response.json({
			access_token: "fresh",
			refresh_token: "rotated",
			expires_in: 600,
		});
	});
	const provider = new AnthropicProvider({ apiBaseUrl: base });
	const refreshed = await provider.refreshAuth(credentials());
	expect(refreshed.credentials).toMatchObject({ access_token: "fresh", refresh_token: "rotated" });
	expect(seen[0]?.headers.get("anthropic-beta")).toBe("oauth-2025-04-20");
	expect(seen[0]?.headers.get("user-agent")).toBe(
		"anthropic-sdk-typescript/0.94.0 userOAuthProvider",
	);
	invalidGrant = true;
	await expect(provider.refreshAuth(credentials())).rejects.toThrow("sign in to Claude again");
});

test("discovers model contexts and uses the bundled fallback", async () => {
	const base = fakeServer((request) => {
		if (request.path === "/v1/models") {
			return Response.json({
				data: [
					{ id: "claude-opus-4-8", display_name: "Opus" },
					{ id: "new-model", display_name: "New" },
				],
			});
		}
		return new Response("not found", { status: 404 });
	});
	const provider = new AnthropicProvider({ apiBaseUrl: base });
	const listed = await provider.listModels(credentials());
	expect(listed).toMatchObject({
		default_model: "claude-opus-4-8",
		models: [
			{ id: "claude-opus-4-8", context_window: 1_000_000 },
			{ id: "new-model", context_window: 200_000 },
		],
	});
	servers.pop()?.stop(true);
	const offline = new AnthropicProvider({
		apiBaseUrl: "http://127.0.0.1:1",
		requestTimeoutMs: 50,
	});
	expect((await offline.listModels(credentials())).models).toContainEqual(
		expect.objectContaining({ id: "claude-opus-4-8" }),
	);
});

test("streams text and tool calls, refreshes after a 401, and honors cancellation", async () => {
	let messageRequests = 0;
	let betaHeader: string | null = null;
	let userAgent: string | null = null;
	const base = fakeServer((request) => {
		if (request.path === "/v1/oauth/token") {
			return Response.json({
				access_token: "fresh",
				refresh_token: "fresh-refresh",
				expires_in: 600,
			});
		}
		messageRequests += 1;
		betaHeader = request.headers.get("anthropic-beta");
		userAgent = request.headers.get("user-agent");
		if (messageRequests === 1) return new Response("unauthorized", { status: 401 });
		const stream = new ReadableStream<Uint8Array>({
			start(controller) {
				controller.enqueue(
					new TextEncoder().encode(
						"event: content_block_delta\ndata: " +
							'{"index":0,"delta":{"type":"text_delta","text":"hello"}}\n\n' +
							"event: content_block_start\ndata: " +
							'{"index":1,"content_block":{"type":"tool_use",' +
							'"id":"tool-1","name":"read","input":{}}}\n\n' +
							"event: content_block_delta\ndata: " +
							'{"index":1,"delta":{"type":"input_json_delta",' +
							'"partial_json":"{\\"path\\":\\"README\\"}"}}\n\n' +
							'event: content_block_stop\ndata: {"index":1}\n\n',
					),
				);
				request.headers.get("authorization");
				request.signal.addEventListener("abort", () => controller.close());
			},
		});
		return new Response(stream, { headers: { "content-type": "text/event-stream" } });
	});
	const provider = new AnthropicProvider({ apiBaseUrl: base });
	const notifications: Array<Record<string, unknown>> = [];
	const controller = new AbortController();
	const streaming = provider.streamChat(
		{
			model_id: "claude-opus-4-8",
			messages: [{ role: "user", content: "Hi" }],
			tools: [],
			credentials: credentials(),
		},
		9,
		(method, params) => notifications.push({ method, ...params }),
		controller.signal,
	);
	await Bun.sleep(20);
	controller.abort();
	expect(await streaming).toEqual({ metadata: { completed: false, cancelled: true } });
	expect(notifications).toEqual([
		{ method: "text_delta", request_id: 9, delta: "hello" },
		{
			method: "tool_call",
			request_id: 9,
			id: "tool-1",
			name: "read",
			arguments: { path: "README" },
		},
	]);
	expect(messageRequests).toBe(2);
	expect(betaHeader ?? "").toBe(
		"claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14," +
			"context-management-2025-06-27,prompt-caching-scope-2026-01-05," +
			"mid-conversation-system-2026-04-07,advanced-tool-use-2025-11-20," +
			"effort-2025-11-24,extended-cache-ttl-2025-04-11",
	);
	expect(userAgent ?? "").toBe("claude-cli/2.1.165");
});
