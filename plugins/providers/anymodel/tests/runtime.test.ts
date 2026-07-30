import { afterEach, expect, test } from "bun:test";
import { readFileSync } from "node:fs";

const children: Array<ReturnType<typeof Bun.spawn>> = [];
const servers: Array<ReturnType<typeof Bun.serve>> = [];

test("declares the discoverable protocol v2 manifest without usage capability", () => {
	const manifest = JSON.parse(
		readFileSync(`${import.meta.dir}/../misy-plugin.json`, "utf8"),
	) as Record<string, unknown>;
	expect(manifest).toMatchObject({
		id: "anymodel",
		display_name: "AnyModel",
		kind: "provider",
		protocol_version: 2,
		capabilities: { image_input: { version: 1 }, thinking: { version: 1 } },
		auth_methods: [{ id: "api_key", display_name: "AnyModel API key" }],
	});
	expect(JSON.stringify(manifest)).not.toContain('"usage"');
});

afterEach(async () => {
	for (const child of children) child.kill();
	await Promise.all(children.map((child) => child.exited));
	while (servers.length > 0) servers.pop()?.stop(true);
});

test("serves auth, models, chat, and unsupported usage over NDJSON", async () => {
	let model = "";
	let observeChat: () => void = () => undefined;
	let observeRateLimit: () => void = () => undefined;
	const chatStarted = new Promise<void>((resolve) => {
		observeChat = resolve;
	});
	const rateLimitStarted = new Promise<void>((resolve) => {
		observeRateLimit = resolve;
	});
	const api = Bun.serve({
		hostname: "127.0.0.1",
		port: 0,
		async fetch(request) {
			if (request.method === "GET") {
				return Response.json({ data: [{ id: "cx/gpt-5.6-sol" }] });
			}
			const body = (await request.json()) as { model: string };
			if (body.model === "am/rate-limited") {
				observeRateLimit();
				return new Response(null, { status: 429 });
			}
			model = body.model;
			observeChat();
			return new Response(
				'data: {"choices":[{"delta":{"content":"hello"}}]}\n\n' + "data: [DONE]\n\n",
				{ headers: { "content-type": "text/event-stream" } },
			);
		},
	});
	servers.push(api);
	const child = startProvider({
		MISY_ANYMODEL_BASE_URL: `http://127.0.0.1:${api.port}`,
		MISY_ANYMODEL_PUBLIC_CATALOG_URL: "",
	});
	write(
		child,
		'{"jsonrpc":"2.0","id":1,"method":"auth.start",' +
			'"params":{"method":"api_key"}}\n' +
			'{"jsonrpc":"2.0","id":2,"method":"models.list","params":' +
			'{"credentials":{"type":"api_key","api_key":"key"}}}\n' +
			'{"jsonrpc":"2.0","id":3,"method":"chat.start","params":' +
			'{"model_id":"cx/gpt-5.6-sol","messages":[],"tools":[],' +
			'"credentials":{"type":"api_key","api_key":"key"}}}\n' +
			'{"jsonrpc":"2.0","id":5,"method":"chat.start","params":' +
			'{"model_id":"am/rate-limited","messages":[],"tools":[],' +
			'"credentials":{"type":"api_key","api_key":"key"}}}\n' +
			'{"jsonrpc":"2.0","id":4,"method":"usage.get","params":' +
			'{"credentials":{"type":"api_key","api_key":"key"}}}\n',
	);
	await Promise.all([chatStarted, rateLimitStarted]);
	await Bun.sleep(20);
	end(child);
	const stdout = await read(child.stdout);
	const stderr = await read(child.stderr);
	const replies = stdout
		.trim()
		.split("\n")
		.map((line) => JSON.parse(line) as Record<string, unknown>);

	expect(stderr).toBe("");
	expect(model).toBe("cx/gpt-5.6-sol");
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		id: 1,
		result: {
			kind: "prompt",
			fields: [{ id: "api_key", label: "API key", secret: true }],
			session: { method: "api_key" },
		},
	});
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		method: "failed",
		params: {
			request_id: 5,
			message: "AnyModel rate limit exceeded (429); retry later or select another model",
		},
	});
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		id: 5,
		result: { metadata: { completed: false, failed: true } },
	});
	expect(replies).not.toContainEqual(expect.objectContaining({ id: 5, error: expect.anything() }));
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		method: "text_delta",
		params: { request_id: 3, delta: "hello" },
	});
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		id: 4,
		error: { code: -32000, message: "AnyModel usage is not supported" },
	});
});

function startProvider(environment: Record<string, string>): ReturnType<typeof Bun.spawn> {
	const child = Bun.spawn(["bun", "src/index.ts"], {
		cwd: `${import.meta.dir}/..`,
		env: { ...process.env, ...environment },
		stdin: "pipe",
		stdout: "pipe",
		stderr: "pipe",
	});
	children.push(child);
	return child;
}

function write(child: ReturnType<typeof Bun.spawn>, input: string): void {
	const stdin = child.stdin;
	if (stdin === null || stdin === undefined || typeof stdin === "number")
		throw new Error("missing stdin");
	stdin.write(input);
}

function end(child: ReturnType<typeof Bun.spawn>): void {
	const stdin = child.stdin;
	if (stdin === null || stdin === undefined || typeof stdin === "number")
		throw new Error("missing stdin");
	stdin.end();
}

async function read(
	stream: ReadableStream<Uint8Array> | number | null | undefined,
): Promise<string> {
	if (stream === null || stream === undefined || typeof stream === "number") {
		throw new Error("missing process stream");
	}
	return await new Response(stream).text();
}
