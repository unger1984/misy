import { afterEach, expect, test } from "bun:test";
import { KimiProvider } from "../src/provider";

type CapturedRequest = {
	path: string;
	headers: Headers;
	fields: Record<string, string>;
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
				fields: formFields(await request.formData()),
			}),
	});
	servers.push(server);
	return `http://127.0.0.1:${server.port}`;
}

function formFields(form: Iterable<[string, unknown]>): Record<string, string> {
	const fields: Record<string, string> = {};
	for (const [key, value] of form) {
		if (typeof value === "string") fields[key] = value;
	}
	return fields;
}

function provider(
	base: string,
	dataDir = `/tmp/misy-kimi-test-${crypto.randomUUID()}`,
): KimiProvider {
	return new KimiProvider({
		authBaseUrl: base,
		apiBaseUrl: base,
		dataDir,
		requestTimeoutMs: 10_000,
		packageVersion: "test\r\nversion",
	});
}

function session(start: Record<string, unknown>): unknown {
	return start["session"];
}

test("returns a device response and chooses Kimi's complete verification URL", async () => {
	let request: CapturedRequest | undefined;
	const base = fakeServer((captured) => {
		request = captured;
		return Response.json({
			user_code: "CODE-123",
			device_code: "device-code",
			verification_uri: "https://kimi.example/verify",
			verification_uri_complete: "https://kimi.example/verify?code=CODE-123",
			expires_in: 60,
			interval: 1,
		});
	});
	const started = await provider(base).startAuth();

	expect(started).toMatchObject({
		kind: "device",
		url: "https://kimi.example/verify?code=CODE-123",
		user_code: "CODE-123",
	});
	expect(typeof started["expires_at"]).toBe("number");
	expect(request?.path).toBe("/api/oauth/device_authorization");
	expect(request?.fields).toEqual({ client_id: "17e5f671-d194-4dfb-9706-5516cb48c098" });
	expect(request?.headers.get("x-msh-platform")).toBe("kimi_cli");
	expect(request?.headers.get("x-msh-version")).toBe("testversion");
});

test("falls back to Kimi's ordinary verification URL when a complete URL is absent", async () => {
	const base = fakeServer(() =>
		Response.json({
			user_code: "CODE-123",
			device_code: "device-code",
			verification_uri: "https://kimi.example/verify",
		}),
	);

	expect(await provider(base).startAuth()).toMatchObject({
		kind: "device",
		url: "https://kimi.example/verify",
		user_code: "CODE-123",
	});
});

test("polls pending authorization and retains a refresh token when refresh omits it", async () => {
	let polls = 0;
	const base = fakeServer((request) => {
		if (request.path === "/api/oauth/device_authorization") {
			return Response.json({
				user_code: "CODE",
				device_code: "device-code",
				verification_uri: "https://kimi.example/verify",
				expires_in: 20,
				interval: 1,
			});
		}
		if (request.fields["grant_type"] === "refresh_token") {
			expect(request.fields).toMatchObject({
				grant_type: "refresh_token",
				refresh_token: "old-refresh",
				client_id: "17e5f671-d194-4dfb-9706-5516cb48c098",
			});
			return Response.json({ access_token: "fresh", expires_in: 3_600 });
		}
		polls += 1;
		return polls === 1
			? Response.json({ error: "authorization_pending" }, { status: 400 })
			: Response.json({ access_token: "access", refresh_token: "refresh", expires_in: 3_600 });
	});
	const kimi = provider(base);
	const started = await kimi.startAuth();
	const completed = await kimi.completeAuth(session(started));
	const refreshed = await kimi.refreshAuth({
		access_token: "old",
		refresh_token: "old-refresh",
		type: "oauth",
	});

	expect(polls).toBe(2);
	expect(completed.credentials).toMatchObject({ access_token: "access", refresh_token: "refresh" });
	expect(refreshed.credentials).toMatchObject({
		access_token: "fresh",
		refresh_token: "old-refresh",
	});
});

test("slows the device poll interval and surfaces expiry and denial distinctly", async () => {
	const outcomes = ["slow_down", "access_denied"];
	let tokenCalls = 0;
	const base = fakeServer((request) => {
		if (request.path.endsWith("device_authorization")) {
			return Response.json({
				user_code: "CODE",
				device_code: "device-code",
				verification_uri: "https://kimi.example/verify",
				expires_in: 20,
				interval: 1,
			});
		}
		const outcome = outcomes[tokenCalls] ?? "access_denied";
		tokenCalls += 1;
		return Response.json({ error: outcome, interval: 1 }, { status: 400 });
	});
	const kimi = provider(base);
	const started = await kimi.startAuth();
	const before = Date.now();
	await expect(kimi.completeAuth(session(started))).rejects.toThrow("denied");

	expect(tokenCalls).toBe(2);
	expect(Date.now() - before).toBeGreaterThanOrEqual(5_500);

	const expiredBase = fakeServer((request) =>
		request.path.endsWith("device_authorization")
			? Response.json({
					user_code: "CODE",
					device_code: "device-code",
					verification_uri: "https://kimi.example/verify",
				})
			: Response.json({ error: "expired_token" }, { status: 400 }),
	);
	const expired = provider(expiredBase);
	await expect(expired.completeAuth(session(await expired.startAuth()))).rejects.toThrow("expired");
}, 10_000);

test("uses an ephemeral device id when its provider data directory cannot be written", async () => {
	let deviceId = "";
	const base = fakeServer((request) => {
		deviceId = request.headers.get("x-msh-device-id") ?? "";
		return Response.json({
			user_code: "CODE",
			device_code: "device-code",
			verification_uri: "https://kimi.example/verify",
		});
	});
	await provider(base, "/dev/null/kimi").startAuth();

	expect(deviceId).toMatch(/^[a-f0-9]{32}$/);
});

test("reports expired sessions after their device code lifetime", async () => {
	const base = fakeServer((request) =>
		request.path.endsWith("device_authorization")
			? Response.json({
					user_code: "CODE",
					device_code: "device-code",
					verification_uri: "https://kimi.example/verify",
					expires_in: 1,
					interval: 1,
				})
			: Response.json({ error: "authorization_pending" }, { status: 400 }),
	);
	const kimi = provider(base);
	await expect(kimi.completeAuth(session(await kimi.startAuth()))).rejects.toThrow("timed out");
});
