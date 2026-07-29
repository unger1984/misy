/**
 * Transport contract tests for the provider SDK.
 *
 * Every test spawns the fixture adapter as a real subprocess and speaks NDJSON JSON-RPC to it,
 * because framing, envelope validation, cancellation, and the EOF lifecycle only exist at the
 * process boundary.
 */
import { afterEach, expect, test } from "bun:test";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

type Reply = {
	jsonrpc?: string;
	id?: unknown;
	result?: unknown;
	error?: { code: number; message: string };
	method?: string;
	params?: unknown;
};

const children: Array<ReturnType<typeof Bun.spawn>> = [];
const tempDirs: string[] = [];

afterEach(async () => {
	for (const child of children.splice(0)) child.kill();
	while (tempDirs.length > 0) {
		const dir = tempDirs.pop();
		if (dir !== undefined) rmSync(dir, { recursive: true, force: true });
	}
});

function startFixture(environment: Record<string, string> = {}): ReturnType<typeof Bun.spawn> {
	const child = Bun.spawn(["bun", "tests/fixture-adapter.ts"], {
		cwd: `${import.meta.dir}/..`,
		env: { ...process.env, ...environment },
		stdin: "pipe",
		stdout: "pipe",
		stderr: "pipe",
	});
	children.push(child);
	return child;
}

async function roundTrip(
	input: string,
	environment: Record<string, string> = {},
): Promise<{ replies: Reply[]; stderr: string }> {
	const child = startFixture(environment);
	write(child, input);
	end(child);
	const replies = await readReplies(child);
	const stderr = await readStderr(child);
	return { replies, stderr };
}

function write(child: ReturnType<typeof Bun.spawn>, input: string): void {
	if (!child.stdin || typeof child.stdin === "number") throw new Error("fixture stdin unavailable");
	child.stdin.write(input);
}

function end(child: ReturnType<typeof Bun.spawn>): void {
	if (!child.stdin || typeof child.stdin === "number") throw new Error("fixture stdin unavailable");
	child.stdin.end();
}

async function readStdout(child: ReturnType<typeof Bun.spawn>): Promise<string> {
	if (!child.stdout || typeof child.stdout === "number") {
		throw new Error("fixture stdout unavailable");
	}
	return await new Response(child.stdout).text();
}

async function readStderr(child: ReturnType<typeof Bun.spawn>): Promise<string> {
	if (!child.stderr || typeof child.stderr === "number") {
		throw new Error("fixture stderr unavailable");
	}
	return await new Response(child.stderr).text();
}

async function readReplies(child: ReturnType<typeof Bun.spawn>): Promise<Reply[]> {
	const stdout = await readStdout(child);
	return stdout
		.trim()
		.split("\n")
		.filter((line) => line.length > 0)
		.map((line) => JSON.parse(line) as Reply);
}

test("frames multiple requests per write and a request split across writes", async () => {
	const child = startFixture();
	write(
		child,
		'{"jsonrpc":"2.0","id":1,"method":"auth.logout","params":{}}\n' +
			'{"jsonrpc":"2.0","id":2,"method":"auth.log',
	);
	write(child, 'out","params":{}}\n');
	end(child);
	// auth.logout resolves synchronously, so reply order matches request order here.
	const replies = await readReplies(child);
	expect(replies).toEqual([
		{ jsonrpc: "2.0", id: 1, result: {} },
		{ jsonrpc: "2.0", id: 2, result: {} },
	]);
});

test("answers parse, invalid-request, and unknown-method errors with protocol codes", async () => {
	const { replies, stderr } = await roundTrip(
		"not-json\n" +
			'{"id":9,"method":"auth.logout"}\n' +
			'"just a string"\n' +
			'{"jsonrpc":"2.0","id":1,"method":"unknown.method","params":{}}\n',
	);
	expect(stderr).toBe("");
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		id: null,
		error: { code: -32700, message: "Parse error" },
	});
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		id: 9,
		error: { code: -32600, message: "Invalid Request" },
	});
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		id: null,
		error: { code: -32600, message: "Invalid Request" },
	});
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		id: 1,
		error: { code: -32601, message: "Method not found" },
	});
});

test("drops requests without an id as notifications", async () => {
	const { replies } = await roundTrip(
		'{"jsonrpc":"2.0","method":"auth.logout","params":{}}\n' +
			'{"jsonrpc":"2.0","id":3,"method":"auth.logout","params":{}}\n',
	);
	expect(replies).toEqual([{ jsonrpc: "2.0", id: 3, result: {} }]);
});

test("passes optional credentials to auth.status and models.list", async () => {
	const { replies } = await roundTrip(
		'{"jsonrpc":"2.0","id":1,"method":"auth.status","params":{}}\n' +
			'{"jsonrpc":"2.0","id":2,"method":"models.list","params":{}}\n' +
			'{"jsonrpc":"2.0","id":3,"method":"auth.status","params":' +
			'{"credentials":{"access_token":"t","type":"oauth","extra":1}}}\n',
	);
	expect(replies).toContainEqual({ jsonrpc: "2.0", id: 1, result: { authenticated: false } });
	const listing = replies.find((reply) => reply.id === 2);
	expect(listing).toMatchObject({ result: { default_model: "fixture-model" } });
	expect(replies).toContainEqual({ jsonrpc: "2.0", id: 3, result: { authenticated: true } });
});

test("rejects missing credentials on methods that require them", async () => {
	const { replies } = await roundTrip(
		'{"jsonrpc":"2.0","id":1,"method":"auth.refresh","params":{}}\n' +
			'{"jsonrpc":"2.0","id":2,"method":"usage.get","params":{}}\n' +
			'{"jsonrpc":"2.0","id":3,"method":"chat.start","params":' +
			'{"model_id":"fast","messages":[],"tools":[]}}\n',
	);
	for (const id of [1, 2, 3]) {
		expect(replies).toContainEqual({
			jsonrpc: "2.0",
			id,
			error: { code: -32000, message: "Fixture OAuth credentials are required" },
		});
	}
});

test("rejects malformed auth.start, auth.complete, and chat.start params", async () => {
	const { replies } = await roundTrip(
		'{"jsonrpc":"2.0","id":1,"method":"auth.start","params":{"method":42}}\n' +
			'{"jsonrpc":"2.0","id":2,"method":"auth.complete","params":{"session":{},"completion":7}}\n' +
			'{"jsonrpc":"2.0","id":3,"method":"chat.start","params":{"messages":[],"tools":[],' +
			'"credentials":{"access_token":"t","type":"oauth"}}}\n' +
			'{"jsonrpc":"2.0","id":4,"method":"chat.start","params":{"model_id":"fast","messages":[7],' +
			'"tools":[],"credentials":{"access_token":"t","type":"oauth"}}}\n' +
			'{"jsonrpc":"2.0","id":5,"method":"chat.start","params":{"model_id":"fast","messages":[],' +
			'"tools":[{"name":"x"}],"credentials":{"access_token":"t","type":"oauth"}}}\n',
	);
	const messages = new Map(replies.map((reply) => [reply.id, reply.error?.message] as const));
	expect(messages.get(1)).toBe("auth.start method must be a string when provided");
	expect(messages.get(2)).toBe("auth.complete completion must be an object");
	expect(messages.get(3)).toBe("chat.start requires a non-empty model_id");
	expect(messages.get(4)).toBe("chat.start requires messages to be an array of objects");
	expect(messages.get(5)).toBe("chat.start tools require string name and description");
});

test("accepts image attachments and rejects malformed message and tool images", async () => {
	const png =
		"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42Y" +
		"AAAAASUVORK5CYII=";
	const validImage = `{"type":"image","media_type":"image/png","data_base64":"${png}"}`;
	const credentials = '{"access_token":"t","type":"oauth"}';
	const { replies } = await roundTrip(
		'{"jsonrpc":"2.0","id":6,"method":"chat.start","params":{"model_id":"fast",' +
			'"messages":[{"role":"user","content":"look","attachments":[' +
			`${validImage}]},{"role":"tool","tool_results":[{"tool_call_id":"call-1",` +
			`"content":"done","attachments":[${validImage}]}]}],"tools":[],` +
			`"credentials":${credentials}}}\n` +
			'{"jsonrpc":"2.0","id":7,"method":"chat.start","params":{"model_id":"fast",' +
			'"messages":[{"role":"user","attachments":[{"type":"image",' +
			'"media_type":"image/jpeg","data_base64":"aW1hZ2U="}]}],"tools":[],' +
			`"credentials":${credentials}}}\n` +
			'{"jsonrpc":"2.0","id":8,"method":"chat.start","params":{"model_id":"fast",' +
			'"messages":[{"role":"tool","tool_results":[{"attachments":[{"type":"image",' +
			'"media_type":"image/png","data_base64":"aW1hZ2U="}]}]}],"tools":[],' +
			`"credentials":${credentials}}}\n`,
	);
	expect(replies).toContainEqual(expect.objectContaining({ id: 6, result: expect.any(Object) }));
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		id: 7,
		error: { code: -32000, message: "chat.start attachments require base64 image/png objects" },
	});
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		id: 8,
		error: { code: -32000, message: "chat.start attachments require base64 image/png objects" },
	});
});

test("rejects an NDJSON input frame larger than 32 MiB and resumes at its newline", async () => {
	const request = '{"jsonrpc":"2.0","id":9,"method":"auth.logout","params":{}}\n';
	const { replies } = await roundTrip(`${" ".repeat(32 * 1024 * 1024 + 1)}\n${request}`);
	expect(replies).toEqual([
		{
			jsonrpc: "2.0",
			id: null,
			error: { code: -32700, message: "Input frame exceeds 32 MiB limit" },
		},
		{ jsonrpc: "2.0", id: 9, result: {} },
	]);
});

test("completes a fast chat and passes rotated credentials through untouched", async () => {
	const { replies } = await roundTrip(
		'{"jsonrpc":"2.0","id":7,"method":"chat.start","params":{"model_id":"fast","messages":[],' +
			'"tools":[],"credentials":{"access_token":"rotated","type":"oauth"}}}\n',
	);
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		method: "text_delta",
		params: { request_id: 7, delta: "fast" },
	});
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		id: 7,
		result: {
			metadata: { completed: true, cancelled: false },
			credentials: { access_token: "rotated", type: "oauth" },
		},
	});
});

test("cancels an in-flight chat by request id without a reply to chat.cancel", async () => {
	const child = startFixture();
	write(
		child,
		'{"jsonrpc":"2.0","id":7,"method":"chat.start","params":{"model_id":"slow","messages":[],' +
			'"tools":[],"credentials":{"access_token":"t","type":"oauth"}}}\n',
	);
	await Bun.sleep(50);
	write(child, '{"jsonrpc":"2.0","method":"chat.cancel","params":{"request_id":7}}\n');
	end(child);
	// The fixture emits the delta before awaiting cancellation, so reply order is stable.
	const replies = await readReplies(child);
	expect(replies).toEqual([
		{ jsonrpc: "2.0", method: "text_delta", params: { request_id: 7, delta: "slow" } },
		{
			jsonrpc: "2.0",
			id: 7,
			result: {
				metadata: { completed: false, cancelled: true },
				credentials: { access_token: "t", type: "oauth" },
			},
		},
	]);
});

test("runs the shutdown hook after stdin closes", async () => {
	const dir = mkdtempSync(join(tmpdir(), "misy-sdk-"));
	tempDirs.push(dir);
	const marker = join(dir, "shutdown-marker");
	const { stderr } = await roundTrip(
		'{"jsonrpc":"2.0","id":1,"method":"auth.logout","params":{}}\n',
		{ FIXTURE_SHUTDOWN_FILE: marker },
	);
	expect(stderr).toBe("");
	expect(existsSync(marker)).toBe(true);
});
