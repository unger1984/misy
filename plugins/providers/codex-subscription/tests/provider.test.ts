import { afterEach, expect, test } from "bun:test";
import { CodexSubscriptionProvider } from "../src/provider";

type Request = { method: string; pathname: string; headers: Headers; body: unknown };

const servers: Array<ReturnType<typeof Bun.serve>> = [];

afterEach(() => {
  while (servers.length > 0) servers.pop()?.stop(true);
});

function fakeServer(handler: (request: Request) => Response | Promise<Response>) {
  const server = Bun.serve({
    port: 0,
    fetch: async (request) =>
      handler({
        method: request.method,
        pathname: new URL(request.url).pathname,
        headers: request.headers,
        body: request.method === "POST"
          ? request.headers.get("content-type")?.includes("application/json") ? await request.json() : Object.fromEntries(await request.formData())
          : undefined,
      }),
  });
  servers.push(server);
  return `http://127.0.0.1:${server.port}`;
}

test("exchanges a localhost PKCE callback for opaque credentials", async () => {
  let tokenBody: Record<string, string> | undefined;
  const issuer = fakeServer(({ pathname, body }) => {
    if (pathname === "/oauth/token") {
      tokenBody = body as Record<string, string>;
      return Response.json({ access_token: "access", refresh_token: "refresh", expires_in: 3600 });
    }
    return new Response("not found", { status: 404 });
  });
  const provider = new CodexSubscriptionProvider({ issuer, clientId: "test-client", codexBaseUrl: issuer });

  const started = await provider.startAuth();
  const authorization = new URL(started.url);
  const callback = authorization.searchParams.get("redirect_uri");
  expect(callback).toBe("http://localhost:1455/auth/callback");
  expect(authorization.searchParams.get("response_type")).toBe("code");
  expect(authorization.searchParams.get("client_id")).toBe("test-client");
  expect(authorization.searchParams.get("scope")).toBe("openid profile email offline_access api.connectors.read api.connectors.invoke");
  expect(authorization.searchParams.get("code_challenge_method")).toBe("S256");
  expect(authorization.searchParams.get("id_token_add_organizations")).toBe("true");
  expect(authorization.searchParams.get("codex_cli_simplified_flow")).toBe("true");
  expect(authorization.searchParams.get("originator")).toBe("codex_cli_rs");
  await fetch(`${callback}?code=browser-code&state=${authorization.searchParams.get("state")}`);

  const completed = await provider.completeAuth(started.session, {});
  expect(completed.credentials).toMatchObject({ access_token: "access", refresh_token: "refresh" });
  expect(tokenBody).toMatchObject({
    grant_type: "authorization_code",
    code: "browser-code",
    client_id: "test-client",
    redirect_uri: callback,
  });
  expect(tokenBody?.code_verifier.length).toBeGreaterThanOrEqual(43);
});

test("falls back to the registered callback port when 1455 is in use", async () => {
  const occupied = Bun.serve({
    hostname: "127.0.0.1",
    port: 1455,
    fetch: () => new Response("occupied"),
  });
  servers.push(occupied);
  const issuer = fakeServer(({ pathname }) => pathname === "/oauth/token"
    ? Response.json({ access_token: "access", refresh_token: "refresh" })
    : new Response("not found", { status: 404 }));
  const provider = new CodexSubscriptionProvider({ issuer, clientId: "test-client", codexBaseUrl: issuer });

  const started = await provider.startAuth();
  expect(new URL(started.url).searchParams.get("redirect_uri"))
    .toBe("http://localhost:1457/auth/callback");
  await provider.completeAuth(started.session, { code: "manual-code" });
});

test("refreshes, lists dynamic models, and maps response streaming tool events", async () => {
  const received: Request[] = [];
  const base = fakeServer(({ pathname, headers, body, method }) => {
    received.push({ pathname, headers, body, method });
    if (pathname === "/oauth/token") {
      return Response.json({ access_token: "new-access", refresh_token: "new-refresh", expires_in: 60 });
    }
    if (pathname === "/models") {
      return Response.json({ models: [{ slug: "gpt-test", display_name: "Test", context_window: 128000 }] });
    }
    if (pathname === "/responses") {
      return new Response([
        'event: response.output_text.delta\n', 'data: {"delta":"Hello"}\n\n',
        'event: response.output_item.done\n', 'data: {"item":{"type":"function_call","call_id":"call-1","name":"read_file","arguments":"{\\"path\\":\\"x\\"}"}}\n\n',
        'event: response.completed\n', 'data: {"response":{"id":"resp-1"}}\n\n',
      ].join(""), { headers: { "content-type": "text/event-stream" } });
    }
    return new Response("not found", { status: 404 });
  });
  const provider = new CodexSubscriptionProvider({ issuer: base, clientId: "test-client", codexBaseUrl: base });
  const credentials = { access_token: "old-access", refresh_token: "refresh" };

  expect(await provider.refreshAuth(credentials)).toMatchObject({ credentials: { access_token: "new-access", refresh_token: "new-refresh" } });
  expect(await provider.listModels(credentials)).toEqual([{ id: "gpt-test", display_name: "Test", context_window: 128000 }]);
  const events: Array<{ method: string; params: Record<string, unknown> }> = [];
  await provider.streamChat({
    model_id: "gpt-test",
    messages: [
      { role: "user", content: "hello" },
      { role: "assistant", content: "", tool_calls: [{ id: "previous", name: "read_file", arguments: { path: "x" } }] },
      { role: "tool", content: "", tool_results: [{ tool_call_id: "previous", content: "ok", is_error: false }] },
    ],
    tools: [{ name: "read_file", description: "Read", input_schema: { type: "object" } }],
    credentials,
  }, 42, (method, params) => events.push({ method, params }));

  expect(events).toEqual([
    { method: "text_delta", params: { request_id: 42, delta: "Hello" } },
    { method: "tool_call", params: { request_id: 42, id: "call-1", name: "read_file", arguments: { path: "x" } } },
    { method: "completed", params: { request_id: 42, metadata: { response: { id: "resp-1" } } } },
  ]);
  const responseRequest = received.find((request) => request.pathname === "/responses");
  expect(responseRequest?.headers.get("authorization")).toBe("Bearer old-access");
  expect(responseRequest?.headers.get("accept")).toBe("text/event-stream");
  expect(responseRequest?.body).toMatchObject({
    model: "gpt-test",
    stream: true,
    tools: [{ type: "function", name: "read_file", description: "Read", parameters: { type: "object" } }],
    input: expect.arrayContaining([
      { type: "function_call", call_id: "previous", name: "read_file", arguments: '{"path":"x"}' },
      { type: "function_call_output", call_id: "previous", output: "ok" },
    ]),
  });
});

test("preserves opaque credentials and the previous refresh token when refresh omits one", async () => {
  const issuer = fakeServer(({ pathname }) => pathname === "/oauth/token"
    ? Response.json({ access_token: "new-access", expires_in: 60, server_field: "new" })
    : new Response("not found", { status: 404 }));
  const provider = new CodexSubscriptionProvider({ issuer, clientId: "test-client", codexBaseUrl: issuer });

  const result = await provider.refreshAuth({
    access_token: "old-access",
    refresh_token: "keep-refresh",
    account_id: "account",
    server_field: "old",
  });

  expect(result.credentials).toMatchObject({
    access_token: "new-access",
    refresh_token: "keep-refresh",
    account_id: "account",
    server_field: "new",
  });
});

test("parses CRLF Responses events for text, tool calls, and completion", async () => {
  const base = fakeServer(({ pathname }) => pathname === "/responses"
    ? new Response([
      'event: response.output_text.delta\r\n', 'data: {"delta":"CRLF"}\r\n\r\n',
      'event: response.output_item.done\r\n', 'data: {"item":{"type":"function_call","call_id":"call-crlf","name":"read_file","arguments":"{\\"path\\":\\"crlf\\"}"}}\r\n\r\n',
      'event: response.completed\r\n', 'data: {"response":{"id":"resp-crlf"}}\r\n\r\n',
    ].join(""), { headers: { "content-type": "text/event-stream" } })
    : new Response("not found", { status: 404 }));
  const provider = new CodexSubscriptionProvider({ issuer: base, clientId: "test-client", codexBaseUrl: base });
  const events: Array<{ method: string; params: Record<string, unknown> }> = [];

  await provider.streamChat({ model_id: "x", messages: [], tools: [], credentials: { access_token: "access" } }, 77,
    (method, params) => events.push({ method, params }));

  expect(events).toEqual([
    { method: "text_delta", params: { request_id: 77, delta: "CRLF" } },
    { method: "tool_call", params: { request_id: 77, id: "call-crlf", name: "read_file", arguments: { path: "crlf" } } },
    { method: "completed", params: { request_id: 77, metadata: { response: { id: "resp-crlf" } } } },
  ]);
});

test("keeps OAuth pending after a mismatched state and accepts a later valid callback", async () => {
  const issuer = fakeServer(({ pathname }) => pathname === "/oauth/token"
    ? Response.json({ access_token: "access", refresh_token: "refresh" })
    : new Response("not found", { status: 404 }));
  const provider = new CodexSubscriptionProvider({ issuer, clientId: "test-client", codexBaseUrl: issuer });
  const started = await provider.startAuth();
  const authorization = new URL(started.url);
  const callback = authorization.searchParams.get("redirect_uri")!;
  const invalid = await fetch(`${callback}?code=nope&state=wrong`);
  expect(invalid.status).toBe(400);
  const valid = await fetch(`${callback}?code=browser-code&state=${authorization.searchParams.get("state")}`);
  expect(valid.status).toBe(200);

  await expect(provider.completeAuth(started.session, {})).resolves.toMatchObject({
    credentials: { access_token: "access", refresh_token: "refresh" },
  });
});

test("reports a failed Responses request", async () => {
  const base = fakeServer(({ pathname }) => pathname === "/responses"
    ? new Response("unauthorized", { status: 401 })
    : new Response("not found", { status: 404 }));
  const provider = new CodexSubscriptionProvider({ issuer: base, clientId: "test-client", codexBaseUrl: base });
  const events: Array<{ method: string; params: Record<string, unknown> }> = [];
  await expect(provider.streamChat({ model_id: "x", messages: [], tools: [], credentials: { access_token: "bad" } }, 9, (method, params) => events.push({ method, params }))).rejects.toThrow("Responses request failed (401)");
  expect(events).toEqual([{ method: "failed", params: { request_id: 9, message: "Responses request failed (401)" } }]);
});
