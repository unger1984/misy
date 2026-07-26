import { afterEach, expect, test } from "bun:test";
import type { ReadableStreamDefaultReader } from "node:stream/web";

const servers: Array<ReturnType<typeof Bun.serve>> = [];
afterEach(() => {
	while (servers.length) {
		servers.pop()?.stop(true);
	}
});

type JsonLineReader = {
	reader: ReadableStreamDefaultReader<Uint8Array<ArrayBuffer>>;
	pending: string;
};

function record(value: unknown): Record<string, unknown> {
	return value !== null && typeof value === "object" && !Array.isArray(value)
		? (value as Record<string, unknown>)
		: {};
}

async function readJsonLine(output: JsonLineReader): Promise<unknown> {
	const decoder = new TextDecoder();
	while (true) {
		const newline = output.pending.indexOf("\n");
		if (newline >= 0) {
			const line = output.pending.slice(0, newline);
			output.pending = output.pending.slice(newline + 1);
			return JSON.parse(line);
		}
		let timeout: ReturnType<typeof setTimeout> | undefined;
		try {
			const chunk = await Promise.race([
				output.reader.read(),
				new Promise<never>((_resolve, reject) => {
					timeout = setTimeout(
						() => reject(new Error("Timed out waiting for provider response")),
						5_000,
					);
				}),
			]);
			if (chunk.done) throw new Error("Provider closed stdout before replying");
			output.pending += decoder.decode(chunk.value, { stream: true });
		} finally {
			if (timeout !== undefined) clearTimeout(timeout);
		}
	}
}

test("streams correlated events and cancels a Responses request", async () => {
	let aborted = false;
	const api = Bun.serve({
		hostname: "127.0.0.1",
		port: 0,
		fetch(request) {
			if (new URL(request.url).pathname !== "/codex/responses")
				return new Response("not found", { status: 404 });
			const stream = new ReadableStream({
				start(controller) {
					controller.enqueue(
						new TextEncoder().encode(
							'event: response.output_text.delta\ndata: {"delta":"first"}\n\n',
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
		env: {
			...process.env,
			MISY_OPENAI_BASE_URL: `http://127.0.0.1:${api.port}`,
			MISY_OPENAI_AUTH_ISSUER: `http://127.0.0.1:${api.port}`,
		},
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
				credentials: { access_token: "opaque", chatgpt_account_id: "account", type: "oauth" },
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
	expect(messages).toContainEqual(
		expect.objectContaining({
			jsonrpc: "2.0",
			id: 7,
			result: { metadata: { completed: false, cancelled: true } },
		}),
	);
	expect(aborted).toBe(true);
});

test("returns JSON-RPC errors without stdout diagnostics", async () => {
	const child = Bun.spawn(["bun", "src/index.ts"], {
		cwd: `${import.meta.dir}/..`,
		stdin: "pipe",
		stdout: "pipe",
		stderr: "pipe",
	});
	child.stdin.write('{"jsonrpc":"2.0","id":1,"method":"unknown","params":{}}\nnot-json\n');
	child.stdin.end();
	const stdout = await new Response(child.stdout).text();
	const replies = stdout
		.trim()
		.split("\n")
		.map((line) => JSON.parse(line));
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		id: 1,
		error: { code: -32601, message: "Method not found" },
	});
	expect(replies.some((reply) => reply.error?.code === -32700)).toBe(true);
});

test("advertises the dynamic catalog default model alongside models.list", async () => {
	const api = Bun.serve({
		hostname: "127.0.0.1",
		port: 0,
		fetch(request) {
			if (new URL(request.url).pathname !== "/codex/models") {
				return new Response("not found", { status: 404 });
			}
			return Response.json({ models: [{ slug: "account-model", context_window: 32_000 }] });
		},
	});
	servers.push(api);
	const child = Bun.spawn(["bun", "src/index.ts"], {
		cwd: `${import.meta.dir}/..`,
		stdin: "pipe",
		stdout: "pipe",
		stderr: "pipe",
		env: {
			...process.env,
			MISY_OPENAI_BASE_URL: `http://127.0.0.1:${api.port}`,
		},
	});
	child.stdin.write(
		'{"jsonrpc":"2.0","id":3,"method":"models.list","params":' +
			'{"credentials":{"access_token":"opaque","chatgpt_account_id":"account","type":"oauth"}}}\n',
	);
	child.stdin.end();
	const stdout = await new Response(child.stdout).text();
	expect(JSON.parse(stdout)).toMatchObject({
		jsonrpc: "2.0",
		id: 3,
		result: { default_model: "account-model" },
	});
});

test("starts a browser flow and keeps auth session separate from completion", async () => {
	const api = Bun.serve({
		hostname: "127.0.0.1",
		port: 0,
		fetch(request) {
			if (new URL(request.url).pathname !== "/oauth/token") {
				return new Response("not found", { status: 404 });
			}
			return Response.json({ access_token: "access", chatgpt_account_id: "account" });
		},
	});
	servers.push(api);
	const child = Bun.spawn(["bun", "src/index.ts"], {
		cwd: `${import.meta.dir}/..`,
		env: {
			...process.env,
			MISY_OPENAI_AUTH_ISSUER: `http://127.0.0.1:${api.port}`,
		},
		stdin: "pipe",
		stdout: "pipe",
		stderr: "pipe",
	});
	const output: JsonLineReader = { reader: child.stdout.getReader(), pending: "" };
	child.stdin.write('{"jsonrpc":"2.0","id":4,"method":"auth.start","params":{}}\n');
	const started = record(await readJsonLine(output));
	const startResult = record(started["result"]);
	const session = record(startResult["session"]);
	expect(startResult).toMatchObject({ kind: "browser" });

	child.stdin.write(
		`${JSON.stringify({
			jsonrpc: "2.0",
			id: 5,
			method: "auth.complete",
			params: {
				session: { id: "wrong" },
				completion: { ...session, code: "manual" },
			},
		})}\n`,
	);
	const rejected = record(await readJsonLine(output));
	expect(rejected).toMatchObject({
		id: 5,
		error: { code: -32000, message: "OAuth session is missing or expired" },
	});

	child.stdin.write(
		`${JSON.stringify({
			jsonrpc: "2.0",
			id: 6,
			method: "auth.complete",
			params: { session, completion: { code: "manual" } },
		})}\n`,
	);
	const completed = record(await readJsonLine(output));
	expect(completed).toMatchObject({
		id: 6,
		result: { credentials: { access_token: "access", chatgpt_account_id: "account" } },
	});
	child.stdin.end();
	await child.exited;
});
