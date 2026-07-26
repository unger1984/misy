import { afterEach, expect, test } from "bun:test";
import { KimiProvider } from "../src/provider";
import type { Credentials } from "../src/types";

type CapturedRequest = {
	path: string;
	headers: Headers;
	body: unknown;
	request: Request;
};

const servers: Array<ReturnType<typeof Bun.serve>> = [];

afterEach(() => {
	while (servers.length > 0) servers.pop()?.stop(true);
});

function fakeServer(handler: (request: CapturedRequest) => Response | Promise<Response>): string {
	const server = Bun.serve({
		hostname: "127.0.0.1",
		port: 0,
		fetch: async (request) =>
			await handler({
				path: new URL(request.url).pathname,
				headers: request.headers,
				body: request.headers.get("content-type")?.includes("application/json")
					? await request.json()
					: undefined,
				request,
			}),
	});
	servers.push(server);
	return `http://127.0.0.1:${server.port}`;
}

function credentials(overrides: Partial<Credentials> = {}): Credentials {
	return { access_token: "access", refresh_token: "refresh", type: "oauth", ...overrides };
}

function provider(base: string): KimiProvider {
	return new KimiProvider({
		authBaseUrl: base,
		apiBaseUrl: base,
		dataDir: `/tmp/misy-kimi-test-${crypto.randomUUID()}`,
		requestTimeoutMs: 10_000,
	});
}

test("discovers Kimi models dynamically and supplies default context windows", async () => {
	let authorization = "";
	const base = fakeServer((request) => {
		authorization = request.headers.get("authorization") ?? "";
		return Response.json({
			data: [{ id: "k3", display_name: "K3", context_length: 1_048_576 }, { id: "legacy" }],
		});
	});
	const models = await provider(base).listModels(credentials());

	expect(authorization).toBe("Bearer access");
	expect(models).toEqual([
		{ id: "k3", display_name: "K3", context_window: 1_048_576 },
		{ id: "legacy", display_name: "legacy", context_window: 262_144 },
	]);
});

test("falls back to bundled models when Kimi's catalog endpoint is unavailable", async () => {
	const base = fakeServer(() => new Response("unavailable", { status: 503 }));
	const models = await provider(base).listModels(credentials());

	expect(models.map((model) => model.id)).toEqual([
		"kimi-for-coding",
		"kimi-for-coding-highspeed",
		"k3",
	]);
});

test("normalizes Kimi Coding subscription usage", async () => {
	let captured: CapturedRequest | undefined;
	const base = fakeServer((request) => {
		captured = request;
		// Mirrors the live /usages payload: quantities are strings, limits carry no name,
		// and resetTime is an ISO timestamp with sub-millisecond precision.
		return Response.json({
			usage: {
				limit: "100",
				used: "33",
				remaining: "67",
				resetTime: "2026-03-08T09:20:45.248979Z",
			},
			limits: [
				{
					window: { duration: 300, timeUnit: "TIME_UNIT_MINUTE" },
					detail: {
						limit: "100",
						used: "2",
						remaining: "98",
						resetTime: "2026-03-07T15:20:45.248979Z",
					},
				},
			],
		});
	});
	const report = await provider(base).usage(credentials());

	expect(captured?.path).toBe("/usages");
	expect(captured?.headers.get("authorization")).toBe("Bearer access");
	expect(report.limits).toHaveLength(2);
	expect(report.limits[0]).toMatchObject({
		label: "Total quota",
		amount: { used: 33, limit: 100, remaining: 67 },
		window: { resets_at: Date.parse("2026-03-08T09:20:45.248979Z") },
	});
	expect(report.limits[1]).toMatchObject({
		label: "5h limit",
		amount: { used: 2, limit: 100, remaining: 98 },
		window: {
			duration_ms: 18_000_000,
			resets_at: Date.parse("2026-03-07T15:20:45.248979Z"),
		},
	});
});

test("refreshes once after a Kimi usage 401", async () => {
	let usageCalls = 0;
	let authorization = "";
	const base = fakeServer((request) => {
		if (request.path === "/api/oauth/token") {
			return Response.json({
				access_token: "fresh",
				refresh_token: "rotated",
				expires_in: 3_600,
			});
		}
		usageCalls += 1;
		authorization = request.headers.get("authorization") ?? "";
		return usageCalls === 1
			? new Response("unauthorized", { status: 401 })
			: Response.json({ usage: { used: 1, limit: 10 } });
	});
	await provider(base).usage(credentials());

	expect(usageCalls).toBe(2);
	expect(authorization).toBe("Bearer fresh");
});

test("streams text and fragmented tools through Kimi's OpenAI request", async () => {
	let captured: CapturedRequest | undefined;
	const base = fakeServer((request) => {
		captured = request;
		return new Response(
			'data: {"choices":[{"delta":{"content":"hello "}}]}\n\n' +
				'data: {"choices":[{"delta":{"content":"world","tool_calls":[' +
				'{"index":0,"id":"call-1","function":{"name":"read_file",' +
				'"arguments":"{\\"path\\":"}}]}}]}\n\n' +
				'data: {"choices":[{"delta":{"tool_calls":[{"index":0,' +
				'"function":{"arguments":"\\"README.md\\"}"}}]}}]}\n\n' +
				"data: [DONE]\n\n",
			{ headers: { "content-type": "text/event-stream" } },
		);
	});
	const notifications: Array<Record<string, unknown>> = [];
	await provider(base).streamChat(
		{
			model_id: "k3",
			messages: [{ role: "user", content: "hello" }],
			tools: [
				{
					name: "read_file",
					description: "Read a file",
					input_schema: { type: "object" },
				},
			],
			credentials: credentials(),
		},
		7,
		(method, params) => notifications.push({ method, ...params }),
		undefined,
	);

	expect(captured?.path).toBe("/chat/completions");
	expect(captured?.body).toMatchObject({
		model: "k3",
		messages: [{ role: "user", content: "hello" }],
		stream: true,
		tools: [
			{
				type: "function",
				function: { name: "read_file", parameters: { type: "object" } },
			},
		],
	});
	expect(notifications).toEqual([
		{ method: "text_delta", request_id: 7, delta: "hello " },
		{ method: "text_delta", request_id: 7, delta: "world" },
		{
			method: "tool_call",
			request_id: 7,
			id: "call-1",
			name: "read_file",
			arguments: { path: "README.md" },
		},
		{ method: "completed", request_id: 7, metadata: { completed: true, finished_by: "done" } },
	]);
});

test("refreshes once after a 401 without exposing the token", async () => {
	let chatCalls = 0;
	const base = fakeServer((request) => {
		if (request.path === "/api/oauth/token") {
			return Response.json({
				access_token: "fresh",
				refresh_token: "fresh-refresh",
				expires_in: 3_600,
			});
		}
		chatCalls += 1;
		return chatCalls === 1
			? new Response("unauthorized", { status: 401 })
			: new Response("data: [DONE]\n\n", { headers: { "content-type": "text/event-stream" } });
	});
	await provider(base).streamChat(
		{ model_id: "k3", messages: [], tools: [], credentials: credentials() },
		4,
		() => {},
		undefined,
	);

	expect(chatCalls).toBe(2);
});

test("stops reading an active Kimi stream when its request is cancelled", async () => {
	const base = fakeServer((request) => {
		const stream = new ReadableStream<Uint8Array>({
			start(controller) {
				controller.enqueue(
					new TextEncoder().encode('data: {"choices":[{"delta":{"content":"first"}}]}\n\n'),
				);
				request.request.signal.addEventListener("abort", () => {
					controller.close();
				});
			},
		});
		return new Response(stream, { headers: { "content-type": "text/event-stream" } });
	});
	const controller = new AbortController();
	const notifications: Array<Record<string, unknown>> = [];
	const streaming = provider(base).streamChat(
		{ model_id: "k3", messages: [], tools: [], credentials: credentials() },
		9,
		(method, params) => notifications.push({ method, ...params }),
		controller.signal,
	);
	await Bun.sleep(25);
	controller.abort();
	await expect(streaming).resolves.toEqual({ metadata: { completed: false, cancelled: true } });

	expect(notifications).toContainEqual({ method: "text_delta", request_id: 9, delta: "first" });
});
