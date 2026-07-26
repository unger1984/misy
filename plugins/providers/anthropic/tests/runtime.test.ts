import { afterEach, expect, test } from "bun:test";

const servers: Array<ReturnType<typeof Bun.serve>> = [];

afterEach(() => {
	while (servers.length > 0) servers.pop()?.stop(true);
});

test("the process streams correlated events and cancels an Anthropic request", async () => {
	let aborted = false;
	const api = Bun.serve({
		hostname: "127.0.0.1",
		port: 0,
		fetch(request) {
			if (new URL(request.url).pathname !== "/v1/messages") {
				return new Response("not found", { status: 404 });
			}
			const stream = new ReadableStream<Uint8Array>({
				start(controller) {
					controller.enqueue(
						new TextEncoder().encode(
							'event: content_block_delta\ndata: {"index":0,"delta":{"text":"first"}}\n\n',
						),
					);
					request.signal.addEventListener("abort", () => {
						aborted = true;
						controller.close();
					});
				},
			});
			return new Response(stream, { headers: { "content-type": "text/event-stream" } });
		},
	});
	servers.push(api);
	const child = Bun.spawn(["bun", "src/index.ts"], {
		cwd: `${import.meta.dir}/..`,
		env: { ...process.env, MISY_ANTHROPIC_API_BASE_URL: `http://127.0.0.1:${api.port}` },
		stdin: "pipe",
		stdout: "pipe",
		stderr: "pipe",
	});
	child.stdin.write(
		`${JSON.stringify({
			jsonrpc: "2.0",
			id: 7,
			method: "chat.start",
			params: {
				model_id: "test",
				messages: [],
				tools: [],
				credentials: { access_token: "opaque", refresh_token: "refresh", type: "oauth" },
			},
		})}\n`,
	);
	await Bun.sleep(50);
	child.stdin.write(
		`${JSON.stringify({ jsonrpc: "2.0", method: "chat.cancel", params: { request_id: 7 } })}\n`,
	);
	child.stdin.end();
	const stdout = await new Response(child.stdout).text();
	const stderr = await new Response(child.stderr).text();
	expect(stderr).toBe("");
	const messages = stdout
		.trim()
		.split("\n")
		.map((line) => JSON.parse(line));
	expect(messages).toContainEqual({
		jsonrpc: "2.0",
		method: "text_delta",
		params: { request_id: 7, delta: "first" },
	});
	expect(messages).toContainEqual({
		jsonrpc: "2.0",
		id: 7,
		result: { metadata: { completed: false, cancelled: true } },
	});
	expect(aborted).toBeTrue();
});

test("the process returns JSON-RPC method and parse errors without diagnostics", async () => {
	const child = Bun.spawn(["bun", "src/index.ts"], {
		cwd: `${import.meta.dir}/..`,
		stdin: "pipe",
		stdout: "pipe",
		stderr: "pipe",
	});
	child.stdin.write('{"jsonrpc":"2.0","id":1,"method":"unknown","params":{}}\nnot-json\n');
	child.stdin.end();
	const stdout = await new Response(child.stdout).text();
	const stderr = await new Response(child.stderr).text();
	const replies = stdout
		.trim()
		.split("\n")
		.map((line) => JSON.parse(line));
	expect(stderr).toBe("");
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		id: 1,
		error: { code: -32601, message: "Method not found" },
	});
	expect(replies.some((reply) => reply.error?.code === -32700)).toBeTrue();
});

test("models.list carries the catalog default", async () => {
	const api = Bun.serve({
		hostname: "127.0.0.1",
		port: 0,
		fetch(request) {
			if (new URL(request.url).pathname === "/v1/models") {
				return Response.json({ data: [{ id: "claude-opus-4-8", display_name: "Opus" }] });
			}
			return new Response("not found", { status: 404 });
		},
	});
	servers.push(api);
	const child = Bun.spawn(["bun", "src/index.ts"], {
		cwd: `${import.meta.dir}/..`,
		env: {
			...process.env,
			MISY_ANTHROPIC_API_BASE_URL: `http://127.0.0.1:${api.port}`,
		},
		stdin: "pipe",
		stdout: "pipe",
		stderr: "pipe",
	});
	child.stdin.write(
		'{"jsonrpc":"2.0","id":3,"method":"models.list","params":' +
			'{"credentials":{"access_token":"opaque","type":"oauth"}}}\n',
	);
	child.stdin.end();
	const stdout = await new Response(child.stdout).text();
	const replies = stdout
		.trim()
		.split("\n")
		.map((line) => JSON.parse(line));
	expect(replies).toContainEqual(
		expect.objectContaining({
			id: 3,
			result: expect.objectContaining({ default_model: "claude-opus-4-8" }),
		}),
	);
});
