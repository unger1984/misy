import { afterEach, expect, test } from "bun:test";
import { OpenAiProvider } from "../src/provider";
import type { Credentials } from "../src/types";

type CapturedRequest = {
	pathname: string;
	headers: Headers;
	body: unknown;
};

const servers: Array<ReturnType<typeof Bun.serve>> = [];

afterEach(() => {
	while (servers.length > 0) {
		servers.pop()?.stop(true);
	}
});

function fakeServer(handler: (request: CapturedRequest) => Response | Promise<Response>): string {
	const server = Bun.serve({
		port: 0,
		fetch: async (request) => {
			const body =
				request.method === "POST" && request.headers.get("content-type")?.includes("json")
					? await request.json()
					: request.method === "POST"
						? Object.fromEntries(await request.formData())
						: undefined;
			return await handler({
				pathname: new URL(request.url).pathname,
				headers: request.headers,
				body,
			});
		},
	});
	servers.push(server);
	return `http://127.0.0.1:${server.port}`;
}

function jwt(accountId: string): string {
	return [
		"e30",
		Buffer.from(
			JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: accountId } }),
		).toString("base64url"),
		"signature",
	].join(".");
}

function credentials(overrides: Partial<Credentials> = {}): Credentials {
	return {
		access_token: "access",
		refresh_token: "refresh",
		chatgpt_account_id: "account-123",
		type: "oauth",
		...overrides,
	};
}

test("exchanges OAuth id_token identity and persists its authentication method", async () => {
	const issuer = fakeServer(({ pathname }) =>
		pathname === "/oauth/token"
			? Response.json({
					access_token: "access",
					refresh_token: "refresh",
					id_token: jwt("workspace"),
				})
			: new Response("not found", { status: 404 }),
	);
	const provider = new OpenAiProvider({ issuer, clientId: "test-client", codexBaseUrl: issuer });

	const started = await provider.startAuth();
	const authorization = new URL(started.url);
	const callback = authorization.searchParams.get("redirect_uri");
	expect(callback).toBe("http://localhost:1455/auth/callback");
	await fetch(`${callback}?code=browser-code&state=${authorization.searchParams.get("state")}`);

	const completed = await provider.completeAuth(started.session, {});
	expect(completed.credentials).toMatchObject({
		access_token: "access",
		chatgpt_account_id: "workspace",
		type: "oauth",
	});
});

test("uses the local catalog without a network request", () => {
	const provider = new OpenAiProvider();
	expect(provider.listModels().map((model) => model.id)).toEqual([
		"gpt-5.6-terra",
		"gpt-5.6-sol",
		"gpt-5.6-luna",
		"gpt-5.5",
		"gpt-5.4",
		"gpt-5.4-mini",
		"gpt-5.3-codex-spark",
	]);
	expect(provider.listModels().map((model) => model.context_window)).toEqual([
		372_000, 372_000, 372_000, 272_000, 272_000, 272_000, 128_000,
	]);
});

test("sends account identity and complete subscription request fields", async () => {
	let received: CapturedRequest | undefined;
	const base = fakeServer((request) => {
		received = request;
		return new Response("event: response.completed\ndata: {}\n\n", {
			headers: { "content-type": "text/event-stream" },
		});
	});
	const provider = new OpenAiProvider({ issuer: base, codexBaseUrl: base });

	await provider.streamChat(
		{
			model_id: "gpt-5.5",
			messages: [
				{ role: "system", content: "System instruction" },
				{ role: "user", content: "Hi" },
				{
					role: "assistant",
					content: "",
					tool_calls: [{ id: "call-1", name: "read_file", arguments: { path: "README.md" } }],
				},
				{
					role: "tool",
					content: "",
					tool_results: [{ tool_call_id: "call-1", content: "contents" }],
				},
			],
			tools: [],
			credentials: credentials(),
		},
		10,
		() => {},
	);

	expect(received?.headers.get("chatgpt-account-id")).toBe("account-123");
	expect(received?.pathname).toBe("/codex/responses");
	expect(received?.body).toMatchObject({
		instructions: "System instruction",
		store: false,
		tool_choice: "auto",
		parallel_tool_calls: true,
		include: ["reasoning.encrypted_content"],
		reasoning: { effort: "medium", summary: "auto" },
		stream_options: { reasoning_summary_delivery: "sequential_cutoff" },
		text: { verbosity: "medium" },
		input: expect.arrayContaining([
			{
				type: "function_call",
				call_id: "call-1",
				name: "read_file",
				arguments: '{"path":"README.md"}',
			},
			{ type: "function_call_output", call_id: "call-1", output: "contents" },
		]),
	});
});

test("replays encrypted reasoning from completed metadata on the next turn", async () => {
	const bodies: unknown[] = [];
	const base = fakeServer((request) => {
		bodies.push(request.body);
		return new Response(
			"event: response.completed\n" +
				'data: {"response":{"output":[{"type":"reasoning",' +
				'"encrypted_content":"opaque-reasoning"}]}}\n\n',
			{ headers: { "content-type": "text/event-stream" } },
		);
	});
	const provider = new OpenAiProvider({ issuer: base, codexBaseUrl: base });
	const first = await provider.streamChat(
		{
			model_id: "gpt-5.5",
			messages: [{ role: "user", content: "first" }],
			tools: [],
			credentials: credentials(),
		},
		13,
		() => {},
	);
	await provider.streamChat(
		{
			model_id: "gpt-5.5",
			messages: [
				{
					role: "assistant",
					content: "answer",
					provider_metadata: { stream: [first.metadata], response: first.metadata },
				},
				{ role: "user", content: "second" },
			],
			tools: [],
			credentials: credentials(),
		},
		14,
		() => {},
	);
	expect(bodies[1]).toMatchObject({
		input: expect.arrayContaining([{ type: "reasoning", encrypted_content: "opaque-reasoning" }]),
	});
});

test("normalizes canonical and already-suffixed Codex base URLs", async () => {
	const paths: string[] = [];
	const base = fakeServer((request) => {
		paths.push(request.pathname);
		return new Response("event: response.completed\ndata: {}\n\n", {
			headers: { "content-type": "text/event-stream" },
		});
	});
	for (const suffix of ["", "/codex", "/codex/responses"]) {
		const provider = new OpenAiProvider({ issuer: base, codexBaseUrl: `${base}${suffix}` });
		await provider.streamChat(
			{ model_id: "gpt-5.5", messages: [], tools: [], credentials: credentials() },
			16,
			() => {},
		);
	}
	expect(paths).toEqual(["/codex/responses", "/codex/responses", "/codex/responses"]);
});

test("includes the server explanation in a failed Responses request", async () => {
	const base = fakeServer(() =>
		Response.json({ error: { message: "Unsupported parameter: text" } }, { status: 400 }),
	);
	const provider = new OpenAiProvider({ issuer: base, codexBaseUrl: base });
	const notifications: Array<Record<string, unknown>> = [];
	await expect(
		provider.streamChat(
			{ model_id: "gpt-5.5", messages: [], tools: [], credentials: credentials() },
			15,
			(method, params) => notifications.push({ method, ...params }),
		),
	).rejects.toThrow("OpenAI Responses request failed (400): Unsupported parameter: text");
	expect(notifications).toContainEqual({
		method: "failed",
		request_id: 15,
		message: "OpenAI Responses request failed (400): Unsupported parameter: text",
	});
});

test("refreshes expiring credentials before the Responses request", async () => {
	const seenTokens: string[] = [];
	const seenAccounts: string[] = [];
	const base = fakeServer((request) => {
		if (request.pathname === "/oauth/token") {
			return Response.json({
				access_token: "fresh",
				refresh_token: "fresh-refresh",
			});
		}
		seenTokens.push(request.headers.get("authorization") ?? "");
		seenAccounts.push(request.headers.get("chatgpt-account-id") ?? "");
		return new Response("event: response.completed\ndata: {}\n\n", {
			headers: { "content-type": "text/event-stream" },
		});
	});
	const provider = new OpenAiProvider({ issuer: base, codexBaseUrl: base });

	await provider.streamChat(
		{ model_id: "gpt-5", messages: [], tools: [], credentials: credentials({ expires_at: 0 }) },
		11,
		() => {},
	);
	expect(seenTokens).toEqual(["Bearer fresh"]);
	expect(seenAccounts).toEqual(["account-123"]);
});

test("retries one 401 after refresh and no more", async () => {
	let responseCalls = 0;
	const base = fakeServer((request) => {
		if (request.pathname === "/oauth/token") {
			return Response.json({
				access_token: "fresh",
				refresh_token: "fresh-refresh",
				id_token: jwt("account-123"),
			});
		}
		responseCalls += 1;
		if (responseCalls === 1) {
			return new Response("unauthorized", { status: 401 });
		}
		return new Response("event: response.completed\ndata: {}\n\n", {
			headers: { "content-type": "text/event-stream" },
		});
	});
	const provider = new OpenAiProvider({ issuer: base, codexBaseUrl: base });

	await provider.streamChat(
		{ model_id: "gpt-5", messages: [], tools: [], credentials: credentials() },
		12,
		() => {},
	);
	expect(responseCalls).toBe(2);
});

test("reports a clear fixed-port error when another OAuth flow owns 1455", async () => {
	const occupied = Bun.serve({
		hostname: "127.0.0.1",
		port: 1455,
		fetch: () => new Response("occupied"),
	});
	servers.push(occupied);
	const provider = new OpenAiProvider();
	await expect(provider.startAuth()).rejects.toThrow("callback port 1455 is already in use");
});

test("rejects authentication methods not declared by the provider", async () => {
	const provider = new OpenAiProvider();
	await expect(provider.startAuth("api_key")).rejects.toThrow(
		"Unsupported OpenAI authentication method: api_key",
	);
});

test("expires abandoned OAuth and frees its callback port", async () => {
	const issuer = fakeServer(({ pathname }) =>
		pathname === "/oauth/token"
			? Response.json({
					access_token: "access",
					refresh_token: "refresh",
					id_token: jwt("account"),
				})
			: new Response("not found", { status: 404 }),
	);
	const provider = new OpenAiProvider({ issuer, codexBaseUrl: issuer, authTimeoutMs: 20 });
	const abandoned = await provider.startAuth();
	await expect(provider.completeAuth(abandoned.session, {})).rejects.toThrow(
		"expired before authentication completed",
	);

	const retried = await provider.startAuth();
	await provider.completeAuth(retried.session, { code: "manual" });
});
