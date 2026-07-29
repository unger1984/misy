import { afterEach, expect, test } from "bun:test";
import { createServer } from "node:net";
import { postOAuthJson } from "../src/oauth-http";
import { AnthropicProvider } from "../src/provider";
import type { Credentials, ImageAttachment } from "../src/types";

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

async function rawServer(response: string, pieceSize?: number): Promise<string> {
	const server = createServer((socket) => {
		socket.once("data", () => {
			if (pieceSize === undefined) {
				socket.end(response);
				return;
			}
			const pieces: string[] = [];
			for (let index = 0; index < response.length; index += pieceSize) {
				pieces.push(response.slice(index, index + pieceSize));
			}
			// The spacing between writes makes TCP coalescing unlikely, so the client
			// really does reassemble the response across many data events.
			const send = (index: number) => {
				const piece = pieces[index];
				if (piece === undefined) {
					socket.end();
					return;
				}
				socket.write(piece, () => setTimeout(() => send(index + 1), 1));
			};
			send(0);
		});
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

test("rejects an OAuth response that grows past the response size limit", async () => {
	const body = `"${"x".repeat(300 * 1024)}"`;
	const base = await rawServer(`HTTP/1.1 200 OK\r\nContent-Length: ${body.length}\r\n\r\n${body}`);
	await expect(
		postOAuthJson(
			new URL("/v1/oauth/token", base),
			{ "content-type": "application/json" },
			"{}",
			5_000,
		),
	).rejects.toThrow("exceeded the 262144-byte limit");
});

test("assembles an OAuth response that arrives in many small pieces", async () => {
	const body = JSON.stringify({ access_token: "pieced", refresh_token: "together" });
	const raw = `HTTP/1.1 200 OK\r\nContent-Length: ${body.length}\r\n\r\n${body}`;
	const base = await rawServer(raw, 7);
	const response = await postOAuthJson(
		new URL("/v1/oauth/token", base),
		{ "content-type": "application/json" },
		"{}",
		5_000,
	);
	expect(response).toEqual({ status: 200, body });
});

test("refuses to send an OAuth request whose header value contains CR/LF", async () => {
	await expect(
		postOAuthJson(
			new URL("http://127.0.0.1:1/v1/oauth/token"),
			{ "x-custom": "fine\r\nInjected: yes" },
			"{}",
			50,
		),
	).rejects.toThrow("CR/LF");
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

test("rejects a pasted code whose state belongs to another authorization", async () => {
	const base = fakeServer(() => Response.json({ access_token: "access" }));
	const provider = new AnthropicProvider({ apiBaseUrl: base, authorizeUrl: `${base}/authorize` });
	const started = await provider.startAuth();
	await expect(
		provider.completeAuth(started.session, { code: "pasted-code#foreign-state" }),
	).rejects.toThrow("state does not match this authorization session");
});

test("rejects a pasted code without a state, as the browser callback does", async () => {
	const base = fakeServer(() => Response.json({ access_token: "access" }));
	const provider = new AnthropicProvider({ apiBaseUrl: base, authorizeUrl: `${base}/authorize` });
	const started = await provider.startAuth();
	await expect(provider.completeAuth(started.session, { code: "pasted-code" })).rejects.toThrow(
		"does not include its state",
	);
});

test("exchanges a pasted code whose state matches the session", async () => {
	let exchange: CapturedRequest | undefined;
	const base = fakeServer((request) => {
		exchange = request;
		return Response.json({ access_token: "access", refresh_token: "refresh", expires_in: 600 });
	});
	const provider = new AnthropicProvider({ apiBaseUrl: base, authorizeUrl: `${base}/authorize` });
	const started = await provider.startAuth();
	const state = new URL(started.url).searchParams.get("state") ?? "";
	const completed = await provider.completeAuth(started.session, {
		code: `pasted-code#${state}`,
	});
	expect(exchange?.body).toMatchObject({
		grant_type: "authorization_code",
		code: "pasted-code",
		state,
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

test("normalizes Claude subscription usage", async () => {
	let received: CapturedRequest | undefined;
	const base = fakeServer((request) => {
		received = request;
		return Response.json({
			five_hour: { utilization: 37.5, resets_at: "2026-11-16T12:00:00Z" },
			seven_day: { utilization: 71 },
		});
	});
	const report = await new AnthropicProvider({ apiBaseUrl: base }).usage(credentials());

	expect(received?.path).toBe("/api/oauth/usage");
	expect(received?.headers.get("authorization")).toBe("Bearer access");
	expect(report.limits.map((limit) => limit.id)).toEqual(["five-hour", "seven-day"]);
	expect(report.limits[0]).toMatchObject({
		amount: { used: 37.5, limit: 100, remaining: 62.5, unit: "percent" },
	});
});

test("refreshes once after a Claude usage 401", async () => {
	let usageCalls = 0;
	let authorization = "";
	const base = fakeServer((request) => {
		if (request.path === "/v1/oauth/token") {
			return Response.json({ access_token: "fresh", refresh_token: "rotated" });
		}
		usageCalls += 1;
		authorization = request.headers.get("authorization") ?? "";
		return usageCalls === 1
			? new Response("unauthorized", { status: 401 })
			: Response.json({ five_hour: { utilization: 10 } });
	});
	await new AnthropicProvider({ apiBaseUrl: base }).usage(credentials());

	expect(usageCalls).toBe(2);
	expect(authorization).toBe("Bearer fresh");
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
			{
				id: "claude-opus-4-8",
				context_window: 1_000_000,
				input_modalities: ["text", "image"],
			},
			{
				id: "new-model",
				context_window: 200_000,
				input_modalities: ["text", "image"],
			},
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

test("maps user and tool-result PNGs to Anthropic base64 image blocks", async () => {
	let received: CapturedRequest | undefined;
	const base = fakeServer((request) => {
		received = request;
		return new Response("event: message_stop\ndata: {}\n\n", {
			headers: { "content-type": "text/event-stream" },
		});
	});
	const image: ImageAttachment = {
		type: "image",
		media_type: "image/png",
		data_base64:
			"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42Y" +
			"AAAAASUVORK5CYII=",
	};

	await new AnthropicProvider({ apiBaseUrl: base }).streamChat(
		{
			model_id: "claude-opus-4-8",
			messages: [
				{ role: "user", content: "plain" },
				{ role: "user", content: "look", attachments: [image] },
				{
					role: "tool",
					tool_results: [{ tool_call_id: "call-1", content: "done", attachments: [image] }],
				},
			],
			tools: [],
			credentials: credentials(),
		},
		10,
		() => {},
	);

	const imageBlock = {
		type: "image",
		source: { type: "base64", media_type: "image/png", data: image.data_base64 },
	};
	expect(received?.body).toMatchObject({
		messages: [
			{ role: "user", content: "plain" },
			{
				role: "user",
				content: [{ type: "text", text: "look" }, imageBlock],
			},
			{
				role: "user",
				content: [
					{
						type: "tool_result",
						tool_use_id: "call-1",
						content: [{ type: "text", text: "done" }, imageBlock],
						is_error: false,
					},
				],
			},
		],
	});
});

test("returns rotated credentials when usage silently refreshes", async () => {
	const base = fakeServer((request) => {
		if (request.path === "/v1/oauth/token") {
			return Response.json({
				access_token: "fresh",
				refresh_token: "rotated",
				expires_in: 600,
			});
		}
		return Response.json({ five_hour: { utilization: 10 } });
	});

	const report = await new AnthropicProvider({ apiBaseUrl: base }).usage(
		credentials({ expires_at: 0 }),
	);

	expect(report.credentials).toMatchObject({ access_token: "fresh", refresh_token: "rotated" });
});

test("omits credentials when usage does not refresh", async () => {
	const base = fakeServer(() => Response.json({ five_hour: { utilization: 10 } }));

	const report = await new AnthropicProvider({ apiBaseUrl: base }).usage(credentials());

	expect(report).not.toHaveProperty("credentials");
});

test("returns rotated credentials when a stream silently refreshes", async () => {
	const base = fakeServer((request) => {
		if (request.path === "/v1/oauth/token") {
			return Response.json({
				access_token: "fresh",
				refresh_token: "rotated",
				expires_in: 600,
			});
		}
		return new Response("event: message_stop\ndata: {}\n\n", {
			headers: { "content-type": "text/event-stream" },
		});
	});

	const result = await new AnthropicProvider({ apiBaseUrl: base }).streamChat(
		{
			model_id: "claude-opus-4-8",
			messages: [],
			tools: [],
			credentials: credentials({ expires_at: 0 }),
		},
		11,
		() => {},
	);

	expect(result.credentials).toMatchObject({ access_token: "fresh", refresh_token: "rotated" });
});

test("omits credentials when a stream does not refresh", async () => {
	const base = fakeServer(
		() =>
			new Response("event: message_stop\ndata: {}\n\n", {
				headers: { "content-type": "text/event-stream" },
			}),
	);

	const result = await new AnthropicProvider({ apiBaseUrl: base }).streamChat(
		{
			model_id: "claude-opus-4-8",
			messages: [],
			tools: [],
			credentials: credentials(),
		},
		12,
		() => {},
	);

	expect(result).not.toHaveProperty("credentials");
});

test("fails a truncated stream payload without leaking a partial delta", async () => {
	const base = fakeServer(
		() =>
			new Response(
				"event: content_block_delta\ndata: " +
					'{"index":0,"delta":{"type":"text_delta","text":"hello"}}\n\n' +
					'event: content_block_delta\ndata: {"index":0,"delta":{"text":"hel\n\n',
				{ headers: { "content-type": "text/event-stream" } },
			),
	);
	const notifications: Array<Record<string, unknown>> = [];
	await expect(
		new AnthropicProvider({ apiBaseUrl: base }).streamChat(
			{ model_id: "claude-opus-4-8", messages: [], tools: [], credentials: credentials() },
			13,
			(method, params) => notifications.push({ method, ...params }),
		),
	).rejects.toThrow("Anthropic Messages stream contained invalid JSON");
	expect(notifications).toEqual([
		{ method: "text_delta", request_id: 13, delta: "hello" },
		{
			method: "failed",
			request_id: 13,
			message: "Anthropic Messages stream contained invalid JSON",
		},
	]);
});

test("fails a stream that ends mid-event instead of completing silently", async () => {
	const base = fakeServer(
		() =>
			new Response(
				"event: content_block_delta\ndata: " +
					'{"index":0,"delta":{"type":"text_delta","text":"hello"}}\n\n' +
					'event: content_block_delta\ndata: {"index":0,"delta":{"text":"hel',
				{ headers: { "content-type": "text/event-stream" } },
			),
	);
	const notifications: Array<Record<string, unknown>> = [];
	await expect(
		new AnthropicProvider({ apiBaseUrl: base }).streamChat(
			{ model_id: "claude-opus-4-8", messages: [], tools: [], credentials: credentials() },
			14,
			(method, params) => notifications.push({ method, ...params }),
		),
	).rejects.toThrow("Anthropic Messages stream ended mid-event");
	expect(notifications).toEqual([
		{ method: "text_delta", request_id: 14, delta: "hello" },
		{ method: "failed", request_id: 14, message: "Anthropic Messages stream ended mid-event" },
	]);
});

test("retries one 401 after refresh and no more", async () => {
	let messageRequests = 0;
	let tokenRequests = 0;
	const base = fakeServer((request) => {
		if (request.path === "/v1/oauth/token") {
			tokenRequests += 1;
			return Response.json({
				access_token: "fresh",
				refresh_token: "fresh-refresh",
				expires_in: 600,
			});
		}
		messageRequests += 1;
		return new Response("unauthorized", { status: 401 });
	});
	const notifications: Array<Record<string, unknown>> = [];
	await expect(
		new AnthropicProvider({ apiBaseUrl: base }).streamChat(
			{ model_id: "claude-opus-4-8", messages: [], tools: [], credentials: credentials() },
			15,
			(method, params) => notifications.push({ method, ...params }),
		),
	).rejects.toThrow("Anthropic Messages request failed (401)");
	expect(messageRequests).toBe(2);
	expect(tokenRequests).toBe(1);
	expect(notifications).toEqual([
		{
			method: "failed",
			request_id: 15,
			message: "Anthropic Messages request failed (401): unauthorized",
		},
	]);
});

test("aborts a hanging Messages request at the configured timeout", async () => {
	const base = fakeServer(
		(request) =>
			new Promise<Response>((resolve) => {
				// The handler never answers on its own; only the client deadline releases it,
				// so a rejection here can only come from the configured request timeout.
				request.signal.addEventListener("abort", () =>
					resolve(new Response("aborted", { status: 500 })),
				);
			}),
	);
	const notifications: Array<Record<string, unknown>> = [];
	await expect(
		new AnthropicProvider({ apiBaseUrl: base, requestTimeoutMs: 50 }).streamChat(
			{ model_id: "claude-opus-4-8", messages: [], tools: [], credentials: credentials() },
			16,
			(method, params) => notifications.push({ method, ...params }),
		),
	).rejects.toThrow();
	expect(notifications.map((entry) => entry["method"])).toEqual(["failed"]);
});

test("rejects a non-object usage response instead of casting it", async () => {
	const base = fakeServer(() => Response.json(["not-an-object"]));
	await expect(new AnthropicProvider({ apiBaseUrl: base }).usage(credentials())).rejects.toThrow(
		"Anthropic usage response must be an object",
	);
});

test("rejects authentication methods not declared by the provider", async () => {
	await expect(new AnthropicProvider().startAuth("api_key")).rejects.toThrow(
		"Unsupported Anthropic authentication method: api_key",
	);
});

test("expires an abandoned authorization instead of waiting forever", async () => {
	const base = fakeServer(() => Response.json({ access_token: "access" }));
	const provider = new AnthropicProvider({ apiBaseUrl: base, authTimeoutMs: 20 });
	const started = await provider.startAuth();
	await expect(provider.completeAuth(started.session, {})).rejects.toThrow(
		"expired before authentication completed",
	);
});
