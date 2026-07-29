import { afterEach, expect, test } from "bun:test";
import { OpenAiProvider } from "../src/provider";
import type { Credentials } from "../src/types";

const servers: Array<ReturnType<typeof Bun.serve>> = [];

afterEach(() => {
	while (servers.length > 0) servers.pop()?.stop(true);
});

function fakeResponsesServer(body: string): string {
	const server = Bun.serve({
		hostname: "127.0.0.1",
		port: 0,
		fetch: () =>
			new Response(body, {
				headers: { "content-type": "text/event-stream" },
			}),
	});
	servers.push(server);
	return `http://127.0.0.1:${server.port}`;
}

function credentials(): Credentials {
	return {
		access_token: "access",
		refresh_token: "refresh",
		chatgpt_account_id: "account-123",
		type: "oauth",
	};
}

test("surfaces nested Responses stream failures without including sibling fields", async () => {
	const base = fakeResponsesServer(
		"event: response.failed\n" +
			'data: {"response":{"error":{"message":"Context window exceeded",' +
			'"authorization":"Bearer secret-token"}}}\n\n',
	);
	const notifications: Array<Record<string, unknown>> = [];
	await expect(
		new OpenAiProvider({ issuer: base, codexBaseUrl: base }).streamChat(
			{ model_id: "gpt-5.5", messages: [], tools: [], credentials: credentials() },
			20,
			(method, params) => notifications.push({ method, ...params }),
		),
	).rejects.toThrow("Context window exceeded");
	expect(notifications).toEqual([
		{ method: "failed", request_id: 20, message: "Context window exceeded" },
	]);
});

test("surfaces nested error-event messages", async () => {
	const base = fakeResponsesServer(
		'event: error\ndata: {"error":{"message":"Backend unavailable"}}\n\n',
	);
	await expect(
		new OpenAiProvider({ issuer: base, codexBaseUrl: base }).streamChat(
			{ model_id: "gpt-5.5", messages: [], tools: [], credentials: credentials() },
			21,
			() => {},
		),
	).rejects.toThrow("Backend unavailable");
});
