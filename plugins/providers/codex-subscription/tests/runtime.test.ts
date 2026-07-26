import { afterEach, expect, test } from "bun:test";

const servers: Array<ReturnType<typeof Bun.serve>> = [];
afterEach(() => { while (servers.length) servers.pop()?.stop(true); });

test("speaks JSON-RPC NDJSON, correlates stream events, and cancels a Responses request", async () => {
  let aborted = false;
  const api = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch(request) {
      if (new URL(request.url).pathname !== "/responses") return new Response("not found", { status: 404 });
      const stream = new ReadableStream({
        start(controller) {
          controller.enqueue(new TextEncoder().encode('event: response.output_text.delta\ndata: {"delta":"first"}\n\n'));
          request.signal.addEventListener("abort", () => { aborted = true; controller.close(); });
        },
      });
      return new Response(stream, { headers: { "content-type": "text/event-stream" } });
    },
  });
  servers.push(api);
  const child = Bun.spawn(["bun", "src/index.ts"], {
    cwd: import.meta.dir + "/..",
    env: { ...process.env, MISY_CODEX_BASE_URL: `http://127.0.0.1:${api.port}`, MISY_CODEX_AUTH_ISSUER: `http://127.0.0.1:${api.port}` },
    stdin: "pipe", stdout: "pipe", stderr: "pipe",
  });
  child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: 7, method: "chat.start", params: { model_id: "test", messages: [], tools: [], credentials: { access_token: "opaque" } } })}\n`);
  await Bun.sleep(50);
  child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "chat.cancel", params: { request_id: 7 } })}\n`);
  child.stdin.end();
  const stdout = await new Response(child.stdout).text();
  const stderr = await new Response(child.stderr).text();
  expect(stderr).toBe("");
  const messages = stdout.trim().split("\n").map((line) => JSON.parse(line));
  expect(messages).toContainEqual({ jsonrpc: "2.0", method: "text_delta", params: { request_id: 7, delta: "first" } });
  expect(messages).toContainEqual(expect.objectContaining({ jsonrpc: "2.0", id: 7, result: { metadata: { completed: false, cancelled: true } } }));
  expect(aborted).toBe(true);
});

test("returns JSON-RPC errors for malformed calls without writing diagnostics to stdout", async () => {
  const child = Bun.spawn(["bun", "src/index.ts"], { cwd: import.meta.dir + "/..", stdin: "pipe", stdout: "pipe", stderr: "pipe" });
  child.stdin.write('{"jsonrpc":"2.0","id":1,"method":"unknown","params":{}}\nnot-json\n');
  child.stdin.end();
  const stdout = await new Response(child.stdout).text();
  const replies = stdout.trim().split("\n").map((line) => JSON.parse(line));
  expect(replies).toContainEqual({ jsonrpc: "2.0", id: 1, error: { code: -32601, message: "Method not found" } });
  expect(replies.some((reply) => reply.error?.code === -32700)).toBe(true);
});
