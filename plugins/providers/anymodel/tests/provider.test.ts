import { afterEach, expect, test } from "bun:test";
import { AnyModelProvider } from "../src/provider";
import type { ApiKeyCredentials, ImageAttachment } from "../src/types";

const servers: Array<ReturnType<typeof Bun.serve>> = [];

afterEach(() => {
	while (servers.length > 0) servers.pop()?.stop(true);
});

function credentials(): ApiKeyCredentials {
	return { type: "api_key", api_key: "not-for-logs" };
}

function fakeServer(handler: (request: Request) => Response | Promise<Response>): string {
	const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: handler });
	servers.push(server);
	return `http://127.0.0.1:${server.port}`;
}

function provider(baseUrl: string): AnyModelProvider {
	return new AnyModelProvider({ baseUrl, requestTimeoutMs: 500 });
}

test("validates the key through the ordered catalog and preserves qualified IDs", async () => {
	let authorization = "";
	const baseUrl = fakeServer((request) => {
		authorization = request.headers.get("authorization") ?? "";
		return Response.json({
			data: [
				{ id: "cx/gpt-5.6-sol", name: "Sol", context_length: 200_000 },
				{ id: "cc/claude-opus-5", input_modalities: ["text", "image"] },
				{ id: " ", name: "discard" },
			],
		});
	});
	const instance = provider(baseUrl);
	const completed = await instance.completeAuth(
		{ method: "api_key" },
		{ api_key: "  not-for-logs " },
	);
	const models = await instance.listModels(completed.credentials);

	expect(authorization).toBe("Bearer not-for-logs");
	expect(models).toEqual([
		{
			id: "cx/gpt-5.6-sol",
			display_name: "Sol",
			context_window: 200_000,
			input_modalities: ["text"],
		},
		{
			id: "cc/claude-opus-5",
			display_name: "cc/claude-opus-5",
			context_window: 128_000,
			input_modalities: ["text", "image"],
		},
	]);
	expect(instance.defaultModel(models)).toBe("cx/gpt-5.6-sol");
});

test("rejects remote catalog failures without reflecting response bodies or API keys", async () => {
	const baseUrl = fakeServer(() => new Response("not-for-logs reflected", { status: 401 }));
	await expect(provider(baseUrl).listModels(credentials())).rejects.toThrow(
		"AnyModel model catalog request failed (401)",
	);
});

test("normalizes image metadata conservatively and rejects empty catalogs", async () => {
	const baseUrl = fakeServer(() =>
		Response.json({
			data: [
				{ id: "unknown", input_modalities: ["text", "audio"] },
				{ id: "duplicate", input_modalities: ["text", "image", "image"] },
			],
		}),
	);
	const models = await provider(baseUrl).listModels(credentials());
	expect(models.map((model) => model.input_modalities)).toEqual([["text"], ["text"]]);

	const empty = fakeServer(() => Response.json({ data: [{ name: "no id" }] }));
	await expect(provider(empty).listModels(credentials())).rejects.toThrow(
		"AnyModel model catalog contained no valid models",
	);
});

test("streams text and fragmented tools with the exact upstream model id", async () => {
	let body: unknown;
	const baseUrl = fakeServer(async (request) => {
		body = await request.json();
		return new Response(
			'data: {"choices":[{"delta":{"content":"hello","tool_calls":[' +
				'{"index":0,"id":"call-1","function":{"name":"read_file",' +
				'"arguments":"{\\"path\\":"}}]}}]}\n\n' +
				'data: {"choices":[{"delta":{"tool_calls":[{"index":0,' +
				'"function":{"arguments":"\\"README.md\\"}"}}]}}]}\n\n' +
				"data: [DONE]\n\n",
			{ headers: { "content-type": "text/event-stream" } },
		);
	});
	const notifications: Array<Record<string, unknown>> = [];
	await provider(baseUrl).streamChat(
		{
			model_id: "cx/gpt-5.6-sol",
			messages: [{ role: "user", content: "inspect", attachments: [] }],
			tools: [{ name: "read_file", description: "Read", input_schema: { type: "object" } }],
			credentials: credentials(),
		},
		7,
		(method, params) => notifications.push({ method, ...params }),
		undefined,
	);

	expect(body).toMatchObject({ model: "cx/gpt-5.6-sol", stream: true });
	expect(notifications).toContainEqual({
		method: "tool_call",
		request_id: 7,
		id: "call-1",
		name: "read_file",
		arguments: { path: "README.md" },
	});
});

test("maps core history to strict OpenAI chat messages", async () => {
	let body: unknown;
	const baseUrl = fakeServer(async (request) => {
		body = await request.json();
		return new Response("data: [DONE]\n\n", {
			headers: { "content-type": "text/event-stream" },
		});
	});
	await provider(baseUrl).streamChat(
		{
			model_id: "am/glm-5.2",
			messages: [
				{
					role: "system",
					content: "instructions",
					tool_calls: [],
					tool_results: [],
					provider_metadata: null,
				},
				{
					role: "user",
					content: "run it",
					tool_calls: [],
					tool_results: [],
					provider_metadata: null,
					attachments: [],
				},
				{
					role: "assistant",
					content: "",
					tool_calls: [{ id: "call-1", name: "SetTodoList", arguments: { todos: [] } }],
					tool_results: [],
					provider_metadata: { ignored: true },
				},
				{
					role: "tool",
					content: "",
					tool_calls: [],
					tool_results: [{ tool_call_id: "call-1", content: "updated", is_error: false }],
					provider_metadata: null,
				},
			],
			tools: [],
			credentials: credentials(),
		},
		8,
		() => undefined,
		undefined,
	);

	expect(body).toMatchObject({
		messages: [
			{ role: "system", content: "instructions" },
			{ role: "user", content: "run it" },
			{
				role: "assistant",
				content: "",
				tool_calls: [
					{
						id: "call-1",
						type: "function",
						function: { name: "SetTodoList", arguments: '{"todos":[]}' },
					},
				],
			},
			{ role: "tool", tool_call_id: "call-1", content: "updated" },
		],
	});
	expect(JSON.stringify(body)).not.toContain("provider_metadata");
	expect(JSON.stringify(body)).not.toContain("tool_results");
});

test("maps user and tool-result images to OpenAI content parts", async () => {
	let body: unknown;
	const baseUrl = fakeServer(async (request) => {
		body = await request.json();
		return new Response("data: [DONE]\n\n", {
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
	await provider(baseUrl).streamChat(
		{
			model_id: "cc/vision",
			messages: [
				{ role: "user", content: "look", attachments: [image] },
				{
					role: "tool",
					tool_results: [{ tool_call_id: "call-1", content: "done", attachments: [image] }],
				},
			],
			tools: [],
			credentials: credentials(),
		},
		8,
		() => undefined,
		undefined,
	);

	expect(JSON.stringify(body)).toContain("data:image/png;base64,");
	expect(JSON.stringify(body)).not.toContain("attachments");
});

test("sanitizes reflected secrets and emits one failed notification", async () => {
	const baseUrl = fakeServer(
		() =>
			new Response('data: {"error":{"message":"authorization: Bearer not-for-logs"}}\n\n', {
				headers: { "content-type": "text/event-stream" },
			}),
	);
	const notifications: Array<Record<string, unknown>> = [];
	const result = await provider(baseUrl).streamChat(
		{ model_id: "cx/model", messages: [], tools: [], credentials: credentials() },
		9,
		(method, params) => notifications.push({ method, ...params }),
		undefined,
	);
	expect(result).toEqual({ metadata: { completed: false, failed: true } });
	expect(JSON.stringify(notifications)).not.toContain("not-for-logs");
	expect(notifications.filter((event) => event["method"] === "failed")).toHaveLength(1);
});

test("reports a rate limit once with an actionable message", async () => {
	const baseUrl = fakeServer(() => new Response(null, { status: 429 }));
	const notifications: Array<Record<string, unknown>> = [];
	const result = await provider(baseUrl).streamChat(
		{ model_id: "am/glm-5.2", messages: [], tools: [], credentials: credentials() },
		10,
		(method, params) => notifications.push({ method, ...params }),
		undefined,
	);

	expect(result).toEqual({ metadata: { completed: false, failed: true } });
	expect(notifications).toEqual([
		{
			method: "failed",
			request_id: 10,
			message: "AnyModel rate limit exceeded (429); retry later or select another model",
		},
	]);
});

test("stops an active stream when the caller cancels", async () => {
	const baseUrl = fakeServer((request) => {
		const stream = new ReadableStream<Uint8Array>({
			start(controller) {
				controller.enqueue(
					new TextEncoder().encode('data: {"choices":[{"delta":{"content":"first"}}]}\n\n'),
				);
				request.signal.addEventListener("abort", () => controller.close());
			},
		});
		return new Response(stream, { headers: { "content-type": "text/event-stream" } });
	});
	const controller = new AbortController();
	const notifications: Array<Record<string, unknown>> = [];
	const streaming = provider(baseUrl).streamChat(
		{ model_id: "cx/model", messages: [], tools: [], credentials: credentials() },
		10,
		(method, params) => notifications.push({ method, ...params }),
		controller.signal,
	);
	await Bun.sleep(25);
	controller.abort();
	await expect(streaming).resolves.toEqual({ metadata: { completed: false, cancelled: true } });
	expect(notifications).not.toContainEqual(expect.objectContaining({ method: "failed" }));
});

test("completes immediately after DONE even when the upstream connection stays open", async () => {
	const baseUrl = fakeServer(() => {
		const stream = new ReadableStream<Uint8Array>({
			start(controller) {
				controller.enqueue(new TextEncoder().encode("data: [DONE]\n\n"));
			},
		});
		return new Response(stream, { headers: { "content-type": "text/event-stream" } });
	});

	const result = await Promise.race([
		provider(baseUrl).streamChat(
			{ model_id: "cx/model", messages: [], tools: [], credentials: credentials() },
			11,
			() => undefined,
			undefined,
		),
		Bun.sleep(200).then(() => {
			throw new Error("stream did not stop after DONE");
		}),
	]);
	expect(result).toEqual({ metadata: { completed: true, finished_by: "done" } });
});

test("does not treat an absent finish reason as a terminal event", async () => {
	const baseUrl = fakeServer(() => {
		const stream = new ReadableStream<Uint8Array>({
			start(controller) {
				controller.enqueue(
					new TextEncoder().encode('data: {"choices":[{"delta":{"content":"first"}}]}\n\n'),
				);
				setTimeout(() => {
					controller.enqueue(
						new TextEncoder().encode(
							'data: {"choices":[{"delta":{"content":"second"},' + '"finish_reason":"stop"}]}\n\n',
						),
					);
					controller.close();
				}, 20);
			},
		});
		return new Response(stream, { headers: { "content-type": "text/event-stream" } });
	});
	const notifications: Array<Record<string, unknown>> = [];
	await provider(baseUrl).streamChat(
		{ model_id: "cx/model", messages: [], tools: [], credentials: credentials() },
		12,
		(method, params) => notifications.push({ method, ...params }),
		undefined,
	);
	expect(notifications).toContainEqual({ method: "text_delta", request_id: 12, delta: "second" });
});

test("reports an oversized unterminated SSE frame once", async () => {
	const baseUrl = fakeServer(
		() =>
			new Response(`data: ${"x".repeat(1024 * 1024 + 1)}`, {
				headers: { "content-type": "text/event-stream" },
			}),
	);
	const notifications: Array<Record<string, unknown>> = [];
	const result = await provider(baseUrl).streamChat(
		{ model_id: "cx/model", messages: [], tools: [], credentials: credentials() },
		13,
		(method, params) => notifications.push({ method, ...params }),
		undefined,
	);

	expect(result).toEqual({ metadata: { completed: false, failed: true } });
	expect(notifications).toEqual([
		{
			method: "failed",
			request_id: 13,
			message: "AnyModel chat stream frame exceeded its size limit",
		},
	]);
});
