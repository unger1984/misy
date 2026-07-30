import { afterEach, expect, test } from "bun:test";
import { readFileSync } from "node:fs";

const children: Array<ReturnType<typeof Bun.spawn>> = [];
const servers: Array<ReturnType<typeof Bun.serve>> = [];

test("declares thinking capability for K3 effort selection", () => {
	const manifest = JSON.parse(
		readFileSync(`${import.meta.dir}/../misy-plugin.json`, "utf8"),
	) as Record<string, unknown>;
	expect(manifest).toMatchObject({
		capabilities: { thinking: { version: 1 } },
	});
});

afterEach(async () => {
	for (const child of children) child.kill();
	await Promise.all(children.map((child) => child.exited));
	while (servers.length > 0) servers.pop()?.stop(true);
});

function startProvider(environment: Record<string, string> = {}): ReturnType<typeof Bun.spawn> {
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

test("returns protocol errors without stderr output", async () => {
	const child = startProvider();
	write(child, '{"jsonrpc":"2.0","id":1,"method":"unknown","params":{}}\nnot-json\n');
	end(child);
	const stdout = await readStdout(child);
	const stderr = await readStderr(child);
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
	expect(replies).toContainEqual({
		jsonrpc: "2.0",
		id: null,
		error: { code: -32700, message: "Parse error" },
	});
});

test("advertises a default model alongside the offline bundled catalog", async () => {
	const child = startProvider();
	write(child, '{"jsonrpc":"2.0","id":3,"method":"models.list","params":{}}\n');
	end(child);
	const stdout = await readStdout(child);

	const reply: unknown = JSON.parse(stdout);
	expect(reply).toMatchObject({
		jsonrpc: "2.0",
		id: 3,
		result: { default_model: "kimi-for-coding" },
	});
	if (!isReply(reply)) throw new Error("models.list did not return an object");
	expect(
		reply.result.models.some(
			(model) =>
				model !== null &&
				typeof model === "object" &&
				"id" in model &&
				model.id === "kimi-for-coding",
		),
	).toBe(true);
});

test("advertises the first live model when Kimi's preferred default is absent", async () => {
	const api = Bun.serve({
		hostname: "127.0.0.1",
		port: 0,
		fetch: () =>
			Response.json({
				data: [
					{ id: "live-k3", display_name: "Live K3", context_length: 1_048_576 },
					{ id: "live-k2", display_name: "Live K2", context_length: 262_144 },
				],
			}),
	});
	servers.push(api);
	const child = startProvider({ MISY_KIMI_API_BASE_URL: `http://127.0.0.1:${api.port}` });
	write(
		child,
		'{"jsonrpc":"2.0","id":4,"method":"models.list","params":' +
			'{"credentials":{"access_token":"opaque","type":"oauth"}}}\n',
	);
	end(child);
	const reply: unknown = JSON.parse(await readStdout(child));

	expect(reply).toMatchObject({
		jsonrpc: "2.0",
		id: 4,
		result: { default_model: "live-k3", models: [{ id: "live-k3" }, { id: "live-k2" }] },
	});
});

test("passes an environment-overridden client id to device authorization", async () => {
	let observeClientId: (clientId: string) => void = () => undefined;
	const observedClientId = new Promise<string>((resolve) => {
		observeClientId = resolve;
	});
	const auth = Bun.serve({
		hostname: "127.0.0.1",
		port: 0,
		async fetch(request) {
			const fields = await request.formData();
			observeClientId(String(fields.get("client_id")));
			return Response.json({
				user_code: "CODE",
				device_code: "device-code",
				verification_uri: "https://kimi.example/verify",
				expires_in: 60,
			});
		},
	});
	servers.push(auth);
	const child = startProvider({
		MISY_KIMI_AUTH_BASE_URL: `http://127.0.0.1:${auth.port}`,
		MISY_KIMI_CLIENT_ID: "local-test-client",
	});
	write(child, '{"jsonrpc":"2.0","id":5,"method":"auth.start","params":{"method":"oauth"}}\n');

	expect(await observedClientId).toBe("local-test-client");
});

function write(child: ReturnType<typeof Bun.spawn>, input: string): void {
	if (!child.stdin || typeof child.stdin === "number")
		throw new Error("test child stdin is unavailable");
	child.stdin.write(input);
}

function end(child: ReturnType<typeof Bun.spawn>): void {
	if (!child.stdin || typeof child.stdin === "number")
		throw new Error("test child stdin is unavailable");
	child.stdin.end();
}

async function readStdout(child: ReturnType<typeof Bun.spawn>): Promise<string> {
	if (!child.stdout || typeof child.stdout === "number")
		throw new Error("test child stdout is unavailable");
	return await new Response(child.stdout).text();
}

async function readStderr(child: ReturnType<typeof Bun.spawn>): Promise<string> {
	if (!child.stderr || typeof child.stderr === "number")
		throw new Error("test child stderr is unavailable");
	return await new Response(child.stderr).text();
}

function isReply(value: unknown): value is { result: { models: unknown[] } } {
	if (value === null || typeof value !== "object" || !("result" in value)) return false;
	const result = value.result;
	return (
		result !== null &&
		typeof result === "object" &&
		"models" in result &&
		Array.isArray(result.models)
	);
}
