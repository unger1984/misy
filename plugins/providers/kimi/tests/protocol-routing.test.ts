import { afterEach, expect, test } from "bun:test";
import { KimiProvider } from "../src/provider";
import type { Credentials, ImageAttachment } from "../src/types";

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
			handler({
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

function credentials(): Credentials {
	return { access_token: "access", refresh_token: "refresh", type: "oauth" };
}

test("routes live model images through the catalog-selected Kimi wire protocol", async () => {
	const requests: CapturedRequest[] = [];
	const base = fakeServer((request) => {
		requests.push(request);
		if (request.path === "/models") {
			return Response.json({
				data: [
					{ id: "live-openai", supports_image_in: true, protocol: null },
					{ id: "live-anthropic", supports_image_in: true, protocol: "anthropic" },
					{ id: "legacy-anthropic", supports_image_in: true },
				],
			});
		}
		if (request.path === "/chat/completions") {
			return new Response("data: [DONE]\n\n", {
				headers: { "content-type": "text/event-stream" },
			});
		}
		return new Response("event: message_stop\ndata: {}\n\n", {
			headers: { "content-type": "text/event-stream" },
		});
	});
	const provider = new KimiProvider({
		authBaseUrl: base,
		apiBaseUrl: base,
		dataDir: `/tmp/misy-kimi-test-${crypto.randomUUID()}`,
	});
	const models = await provider.listModels(credentials());
	const image: ImageAttachment = {
		type: "image",
		media_type: "image/png",
		data_base64:
			"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42Y" +
			"AAAAASUVORK5CYII=",
	};
	for (const modelId of ["live-openai", "live-anthropic", "legacy-anthropic"]) {
		await provider.streamChat(
			{
				model_id: modelId,
				messages: [{ role: "user", content: "look", attachments: [image] }],
				tools: [],
				credentials: credentials(),
			},
			10,
			() => {},
			undefined,
		);
	}

	expect(models.every((model) => !("protocol" in model))).toBe(true);
	expect(requests.slice(1).map((request) => request.path)).toEqual([
		"/chat/completions",
		"/messages",
		"/messages",
	]);
	expect(requests[1]?.body).toEqual(openAiBody(image));
	expect(requests[2]?.body).toEqual(anthropicBody("live-anthropic", image));
	expect(requests[3]?.body).toEqual(anthropicBody("legacy-anthropic", image));
	for (const request of requests.slice(2)) {
		expect(request.headers.get("anthropic-version")).toBe("2023-06-01");
	}
});

test("resolves a cached live model protocol before its first chat after restart", async () => {
	const paths: string[] = [];
	const base = fakeServer((request) => {
		paths.push(request.path);
		if (request.path === "/models") {
			return Response.json({ data: [{ id: "kimi-for-coding", protocol: null }] });
		}
		return new Response("data: [DONE]\n\n", {
			headers: { "content-type": "text/event-stream" },
		});
	});
	const provider = new KimiProvider({
		authBaseUrl: base,
		apiBaseUrl: base,
		dataDir: `/tmp/misy-kimi-test-${crypto.randomUUID()}`,
	});

	await provider.streamChat(
		{
			model_id: "kimi-for-coding",
			messages: [{ role: "user", content: "hello" }],
			tools: [],
			credentials: credentials(),
		},
		11,
		() => {},
		undefined,
	);

	expect(paths).toEqual(["/models", "/chat/completions"]);
});

test("does not promote a public fallback catalog to authoritative routing", async () => {
	const paths: string[] = [];
	let modelCalls = 0;
	const base = fakeServer((request) => {
		paths.push(request.path);
		if (request.path === "/models") {
			modelCalls += 1;
			return modelCalls === 1
				? new Response("unavailable", { status: 503 })
				: Response.json({ data: [{ id: "kimi-for-coding", protocol: null }] });
		}
		return new Response("data: [DONE]\n\n", {
			headers: { "content-type": "text/event-stream" },
		});
	});
	const provider = new KimiProvider({
		authBaseUrl: base,
		apiBaseUrl: base,
		dataDir: `/tmp/misy-kimi-test-${crypto.randomUUID()}`,
	});
	const models = await provider.listModels(credentials());

	await provider.streamChat(
		{
			model_id: "kimi-for-coding",
			messages: [{ role: "user", content: "hello" }],
			tools: [],
			credentials: credentials(),
		},
		15,
		() => {},
		undefined,
	);

	expect(models[0]?.id).toBe("kimi-for-coding");
	expect(paths).toEqual(["/models", "/models", "/chat/completions"]);
});

test("fails explicitly when a fallback model route is unresolved during an outage", async () => {
	const base = fakeServer(() => new Response("unavailable", { status: 503 }));
	const provider = new KimiProvider({
		authBaseUrl: base,
		apiBaseUrl: base,
		dataDir: `/tmp/misy-kimi-test-${crypto.randomUUID()}`,
	});
	const notifications: Array<Record<string, unknown>> = [];

	await expect(
		provider.streamChat(
			{
				model_id: "kimi-for-coding",
				messages: [{ role: "user", content: "hello" }],
				tools: [],
				credentials: credentials(),
			},
			16,
			(method, params) => notifications.push({ method, ...params }),
			undefined,
		),
	).rejects.toThrow("Kimi models request failed (503)");
	expect(notifications).toEqual([
		{
			method: "failed",
			request_id: 16,
			message: "Kimi models request failed (503)",
		},
	]);
});

test("refreshes once when cached-model protocol discovery returns 401", async () => {
	const paths: string[] = [];
	const modelAuthorizations: string[] = [];
	let modelCalls = 0;
	const base = fakeServer((request) => {
		paths.push(request.path);
		if (request.path === "/api/oauth/token") {
			return Response.json({
				access_token: "fresh",
				refresh_token: "rotated",
				expires_in: 3_600,
			});
		}
		if (request.path === "/models") {
			modelCalls += 1;
			modelAuthorizations.push(request.headers.get("authorization") ?? "");
			return modelCalls === 1
				? new Response("unauthorized", { status: 401 })
				: Response.json({ data: [{ id: "cached-openai", protocol: null }] });
		}
		return new Response("data: [DONE]\n\n", {
			headers: { "content-type": "text/event-stream" },
		});
	});
	const provider = new KimiProvider({
		authBaseUrl: base,
		apiBaseUrl: base,
		dataDir: `/tmp/misy-kimi-test-${crypto.randomUUID()}`,
	});

	const result = await provider.streamChat(
		{
			model_id: "cached-openai",
			messages: [{ role: "user", content: "hello" }],
			tools: [],
			credentials: credentials(),
		},
		13,
		() => {},
		undefined,
	);

	expect(paths).toEqual(["/models", "/api/oauth/token", "/models", "/chat/completions"]);
	expect(modelAuthorizations).toEqual(["Bearer access", "Bearer fresh"]);
	expect(result.credentials).toMatchObject({ access_token: "fresh", refresh_token: "rotated" });
});

test("cancels cached-model protocol discovery with the chat request", async () => {
	let lookupCompleted = false;
	const base = fakeServer(async () => {
		await Bun.sleep(500);
		lookupCompleted = true;
		return Response.json({ data: [{ id: "cached-openai", protocol: null }] });
	});
	const provider = new KimiProvider({
		authBaseUrl: base,
		apiBaseUrl: base,
		dataDir: `/tmp/misy-kimi-test-${crypto.randomUUID()}`,
	});
	const controller = new AbortController();
	const streaming = provider.streamChat(
		{
			model_id: "cached-openai",
			messages: [{ role: "user", content: "hello" }],
			tools: [],
			credentials: credentials(),
		},
		14,
		() => {},
		controller.signal,
	);
	const startedAt = performance.now();
	await Bun.sleep(25);
	controller.abort();

	await expect(streaming).resolves.toEqual({ metadata: { completed: false, cancelled: true } });
	expect(performance.now() - startedAt).toBeLessThan(250);
	expect(lookupCompleted).toBe(false);
});

test("uses the legacy missing-protocol route and normalizes its Anthropic stream", async () => {
	let path = "";
	let body: unknown;
	const base = fakeServer((request) => {
		path = request.path;
		body = request.body;
		if (request.path === "/models") {
			return Response.json({ data: [{ id: "kimi-for-coding" }] });
		}
		return new Response(
			'event: content_block_delta\ndata: {"index":0,"delta":{"text":"hello"}}\n\n' +
				'event: content_block_start\ndata: {"index":1,"content_block":' +
				'{"type":"tool_use","id":"call-1","name":"read_file","input":{}}}\n\n' +
				'event: content_block_delta\ndata: {"index":1,"delta":' +
				'{"partial_json":"{\\"path\\":\\"README.md\\"}"}}\n\n' +
				'event: content_block_stop\ndata: {"index":1}\n\n' +
				'event: message_stop\ndata: {"type":"message_stop"}\n\n',
			{ headers: { "content-type": "text/event-stream" } },
		);
	});
	const provider = new KimiProvider({
		authBaseUrl: base,
		apiBaseUrl: base,
		dataDir: `/tmp/misy-kimi-test-${crypto.randomUUID()}`,
	});
	const notifications: Array<Record<string, unknown>> = [];
	await provider.streamChat(
		{
			model_id: "kimi-for-coding",
			messages: [
				{
					role: "assistant",
					content: "I'll read it",
					tool_calls: [{ id: "call-1", name: "read_file", arguments: { path: "README.md" } }],
				},
				{
					role: "tool",
					tool_results: [{ tool_call_id: "call-1", content: "contents" }],
				},
			],
			tools: [],
			credentials: credentials(),
		},
		12,
		(method, params) => notifications.push({ method, ...params }),
		undefined,
	);

	expect(path).toBe("/messages");
	expect(body).toMatchObject({
		messages: [
			{
				role: "assistant",
				content: [
					{ type: "text", text: "I'll read it" },
					{
						type: "tool_use",
						id: "call-1",
						name: "read_file",
						input: { path: "README.md" },
					},
				],
			},
			{
				role: "user",
				content: [
					{
						type: "tool_result",
						tool_use_id: "call-1",
						content: [{ type: "text", text: "contents" }],
						is_error: false,
					},
				],
			},
		],
	});
	expect(notifications).toEqual([
		{ method: "text_delta", request_id: 12, delta: "hello" },
		{
			method: "tool_call",
			request_id: 12,
			id: "call-1",
			name: "read_file",
			arguments: { path: "README.md" },
		},
		{
			method: "completed",
			request_id: 12,
			metadata: { type: "message_stop" },
		},
	]);
});

test("fails an Anthropic stream that reaches clean EOF before message_stop", async () => {
	const base = fakeServer((request) => {
		if (request.path === "/models") {
			return Response.json({ data: [{ id: "live-anthropic", protocol: "anthropic" }] });
		}
		return new Response(
			'event: content_block_delta\ndata: {"index":0,"delta":{"text":"partial"}}\n\n',
			{ headers: { "content-type": "text/event-stream" } },
		);
	});
	const provider = new KimiProvider({
		authBaseUrl: base,
		apiBaseUrl: base,
		dataDir: `/tmp/misy-kimi-test-${crypto.randomUUID()}`,
	});
	const notifications: Array<Record<string, unknown>> = [];

	await expect(
		provider.streamChat(
			{
				model_id: "live-anthropic",
				messages: [{ role: "user", content: "hello" }],
				tools: [],
				credentials: credentials(),
			},
			17,
			(method, params) => notifications.push({ method, ...params }),
			undefined,
		),
	).rejects.toThrow("Kimi Anthropic stream ended before message_stop");
	expect(notifications).toEqual([
		{ method: "text_delta", request_id: 17, delta: "partial" },
		{
			method: "failed",
			request_id: 17,
			message: "Kimi Anthropic stream ended before message_stop",
		},
	]);
});

function openAiBody(image: ImageAttachment): unknown {
	return {
		model: "live-openai",
		messages: [
			{
				role: "user",
				content: [
					{ type: "text", text: "look" },
					{
						type: "image_url",
						image_url: { url: `data:image/png;base64,${image.data_base64}` },
					},
				],
			},
		],
		tools: [],
		stream: true,
	};
}

function anthropicBody(model: string, image: ImageAttachment): unknown {
	return {
		model,
		max_tokens: 32_000,
		stream: true,
		messages: [
			{
				role: "user",
				content: [
					{ type: "text", text: "look" },
					{
						type: "image",
						source: {
							type: "base64",
							media_type: "image/png",
							data: image.data_base64,
						},
					},
				],
			},
		],
		tools: [],
	};
}
