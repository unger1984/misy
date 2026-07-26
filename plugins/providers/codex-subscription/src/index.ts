import { CodexSubscriptionProvider, type Credentials } from "./provider";

type RpcRequest = { jsonrpc?: string; id?: number | string | null; method?: string; params?: Record<string, unknown> };
const provider = new CodexSubscriptionProvider();
const chats = new Map<number, AbortController>();
const tasks = new Set<Promise<void>>();

function send(message: Record<string, unknown>) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function notification(method: string, params: Record<string, unknown>) {
  send({ jsonrpc: "2.0", method, params });
}

function error(id: RpcRequest["id"], code: number, message: string) {
  send({ jsonrpc: "2.0", id: id ?? null, error: { code, message } });
}

function credentials(params: Record<string, unknown>): Credentials | undefined {
  const value = params.credentials;
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as Credentials : undefined;
}

async function handle(request: RpcRequest): Promise<void> {
  if (request.jsonrpc !== "2.0" || typeof request.method !== "string") {
    error(request.id, -32600, "Invalid Request");
    return;
  }
  const params = request.params ?? {};
  if (request.method === "chat.cancel") {
    const requestId = params.request_id;
    if (typeof requestId !== "number") return;
    chats.get(requestId)?.abort();
    return;
  }
  if (request.id === undefined || request.id === null) return;
  try {
    let result: unknown;
    switch (request.method) {
      case "auth.status": result = provider.authStatus(credentials(params)); break;
      case "auth.start": {
        const started = await provider.startAuth();
        result = { url: started.url, session: started.session };
        break;
      }
      case "auth.complete": {
        const completion = params.completion;
        result = await provider.completeAuth(
          completion && typeof completion === "object" ? completion : {},
          completion && typeof completion === "object" ? completion as Record<string, unknown> : {},
        );
        break;
      }
      case "auth.refresh": {
        const value = credentials(params);
        if (!value) throw new Error("Credentials are required");
        result = await provider.refreshAuth(value);
        break;
      }
      case "auth.logout": result = provider.logout(); break;
      case "models.list": {
        const value = credentials(params);
        if (!value) throw new Error("Credentials are required");
        result = { models: await provider.listModels(value) };
        break;
      }
      case "chat.start": {
        const requestId = typeof request.id === "number" ? request.id : Number(request.id);
        if (!Number.isSafeInteger(requestId)) throw new Error("chat.start requires a numeric JSON-RPC id");
        const value = credentials(params);
        if (!value || typeof params.model_id !== "string" || !Array.isArray(params.messages) || !Array.isArray(params.tools)) {
          throw new Error("chat.start requires credentials, model_id, messages, and tools");
        }
        const controller = new AbortController();
        chats.set(requestId, controller);
        try {
          result = await provider.streamChat({
            model_id: params.model_id,
            messages: params.messages as Array<Record<string, unknown>>,
            tools: params.tools as Array<{ name: string; description: string; input_schema: never }>,
            credentials: value,
          }, requestId, notification, controller.signal);
        } finally {
          chats.delete(requestId);
        }
        break;
      }
      default: error(request.id, -32601, "Method not found"); return;
    }
    send({ jsonrpc: "2.0", id: request.id, result });
  } catch (cause) {
    const message = cause instanceof Error ? cause.message : String(cause);
    error(request.id, -32000, message);
  }
}

async function main() {
  const decoder = new TextDecoder();
  let pending = "";
  for await (const chunk of Bun.stdin.stream()) {
    pending += decoder.decode(chunk, { stream: true });
    let index: number;
    while ((index = pending.indexOf("\n")) >= 0) {
      const line = pending.slice(0, index).trim();
      pending = pending.slice(index + 1);
      if (!line) continue;
      let request: RpcRequest;
      try { request = JSON.parse(line) as RpcRequest; }
      catch { error(null, -32700, "Parse error"); continue; }
      const task = handle(request).finally(() => tasks.delete(task));
      tasks.add(task);
    }
  }
  for (const controller of chats.values()) controller.abort();
  await Promise.allSettled(tasks);
}

void main();
