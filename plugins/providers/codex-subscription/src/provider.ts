export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

export type Credentials = {
  access_token: string;
  refresh_token?: string;
  expires_at?: number;
  chatgpt_account_id?: string;
  [key: string]: Json | undefined;
};

export type ProviderConfig = {
  issuer: string;
  clientId: string;
  codexBaseUrl: string;
  scopes: string[];
  originator: string;
  clientVersion: string;
  authTimeoutMs: number;
};

export type AuthSession = { id: string; url: string; session: { id: string } };
type PendingAuth = {
  verifier: string;
  redirectUri: string;
  code: Promise<{ code?: string; error?: string }>;
  stop: () => void;
};

export type ChatRequest = {
  model_id: string;
  messages: Array<Record<string, unknown>>;
  tools: Array<{ name: string; description: string; input_schema: Json }>;
  credentials: Credentials;
};

export type Notify = (method: string, params: Record<string, unknown>) => void;

const DEFAULT_CONFIG: ProviderConfig = {
  issuer: process.env.MISY_CODEX_AUTH_ISSUER ?? "https://auth.openai.com",
  clientId: process.env.MISY_CODEX_CLIENT_ID ?? "app_EMoamEEZ73f0CkXaXp7hrann",
  codexBaseUrl: process.env.MISY_CODEX_BASE_URL ?? "https://chatgpt.com/backend-api/codex",
  scopes: (process.env.MISY_CODEX_OAUTH_SCOPES ?? "openid profile email offline_access api.connectors.read api.connectors.invoke").split(" ").filter(Boolean),
  originator: process.env.MISY_CODEX_ORIGINATOR ?? "codex_cli_rs",
  clientVersion: process.env.MISY_CODEX_CLIENT_VERSION ?? "0.142.5",
  authTimeoutMs: positiveInteger(process.env.MISY_CODEX_AUTH_TIMEOUT_MS, 300_000),
};

/** Stateless API adapter; temporary OAuth callbacks are retained only in memory. */
export class CodexSubscriptionProvider {
  private readonly config: ProviderConfig;
  private readonly pending = new Map<string, PendingAuth>();

  constructor(config: Partial<ProviderConfig> = {}) {
    this.config = { ...DEFAULT_CONFIG, ...config };
  }

  async startAuth(): Promise<AuthSession> {
    const verifier = randomUrlToken(64);
    const state = randomUrlToken(32);
    let resolveCode!: (result: { code?: string; error?: string }) => void;
    const code = new Promise<{ code?: string; error?: string }>((resolve) => {
      resolveCode = resolve;
    });
    let server: ReturnType<typeof Bun.serve> | undefined;
    server = bindCallbackServer((request) => {
      const callback = new URL(request.url);
      if (callback.pathname !== "/auth/callback") return new Response("Not found", { status: 404 });
      if (callback.searchParams.get("state") !== state) {
        return new Response("OAuth state did not match", { status: 400 });
      }
      const authorizationCode = callback.searchParams.get("code");
      if (!authorizationCode) {
        resolveCode({ error: "OAuth callback did not include an authorization code" });
        return new Response("OAuth code is missing", { status: 400 });
      }
      resolveCode({ code: authorizationCode });
      return new Response("Authentication completed. You may close this window.");
    });
    const redirectUri = `http://localhost:${server.port}/auth/callback`;
    const url = new URL("/oauth/authorize", this.config.issuer);
    url.searchParams.set("response_type", "code");
    url.searchParams.set("client_id", this.config.clientId);
    url.searchParams.set("redirect_uri", redirectUri);
    url.searchParams.set("scope", this.config.scopes.join(" "));
    url.searchParams.set("state", state);
    url.searchParams.set("code_challenge", await pkceChallenge(verifier));
    url.searchParams.set("code_challenge_method", "S256");
    url.searchParams.set("id_token_add_organizations", "true");
    url.searchParams.set("codex_cli_simplified_flow", "true");
    url.searchParams.set("originator", this.config.originator);
    const id = randomUrlToken(18);
    let timeout: ReturnType<typeof setTimeout> | undefined;
    const stop = () => {
      if (timeout) clearTimeout(timeout);
      server?.stop(true);
    };
    this.pending.set(id, { verifier, redirectUri, code, stop });
    timeout = setTimeout(() => {
      if (!this.pending.has(id)) return;
      resolveCode({ error: "OAuth session expired" });
      stop();
      this.pending.delete(id);
    }, this.config.authTimeoutMs);
    return { id, url: url.toString(), session: { id } };
  }

  async completeAuth(session: unknown, completion: Record<string, unknown>): Promise<{ credentials: Credentials }> {
    const sessionId = typeof session === "object" && session !== null && "id" in session
      ? String((session as { id: unknown }).id)
      : String(session);
    const pending = this.pending.get(sessionId);
    if (!pending) throw new Error("OAuth session is missing or expired");
    try {
      const suppliedCode = typeof completion.code === "string" ? completion.code : undefined;
      const callback = suppliedCode ? undefined : await pending.code;
      if (callback?.error) throw new Error(callback.error);
      const credentials = await this.token({
        grant_type: "authorization_code",
        code: suppliedCode ?? callback?.code ?? "",
        redirect_uri: pending.redirectUri,
        code_verifier: pending.verifier,
      });
      return { credentials };
    } finally {
      pending.stop();
      this.pending.delete(sessionId);
    }
  }

  async refreshAuth(credentials: Credentials): Promise<{ credentials: Credentials }> {
    if (!credentials.refresh_token) throw new Error("Credentials do not contain a refresh token");
    const refreshed = await this.token({ grant_type: "refresh_token", refresh_token: credentials.refresh_token });
    return {
      credentials: {
        ...credentials,
        ...refreshed,
        refresh_token: refreshed.refresh_token ?? credentials.refresh_token,
      },
    };
  }

  authStatus(credentials: Credentials | undefined): { authenticated: boolean; expires_at?: number } {
    return credentials?.access_token
      ? { authenticated: true, ...(credentials.expires_at ? { expires_at: credentials.expires_at } : {}) }
      : { authenticated: false };
  }

  logout(): Record<string, never> { return {}; }

  async listModels(credentials: Credentials): Promise<Array<{ id: string; display_name: string; context_window: number }>> {
    const url = new URL("models", withSlash(this.config.codexBaseUrl));
    url.searchParams.set("client_version", this.config.clientVersion);
    const response = await fetch(url, {
      headers: authHeaders(credentials),
    });
    if (!response.ok) throw new Error(await responseError("Models request failed", response));
    const payload = await response.json() as Record<string, unknown>;
    const values = Array.isArray(payload) ? payload : Array.isArray(payload.models) ? payload.models : payload.data;
    if (!Array.isArray(values)) throw new Error("Models response did not contain a models array");
    return values.map((value) => {
      const model = value as Record<string, unknown>;
      const id = pickString(model, ["id", "slug", "model"]);
      if (!id) throw new Error("Model is missing an id");
      return {
        id,
        display_name: pickString(model, ["display_name", "name", "id", "slug"]) ?? id,
        context_window: pickNumber(model, ["context_window", "contextWindow", "context_length"]) ?? 128000,
      };
    });
  }

  async streamChat(request: ChatRequest, requestId: number, notify: Notify, signal?: AbortSignal): Promise<{ metadata: Json }> {
    try {
      const response = await fetch(new URL("responses", withSlash(this.config.codexBaseUrl)), {
        method: "POST",
        signal,
        headers: { ...authHeaders(request.credentials), accept: "text/event-stream", "content-type": "application/json" },
        body: JSON.stringify({
          model: request.model_id,
          stream: true,
          input: responseInput(request.messages),
          tools: request.tools.map((tool) => ({ type: "function", name: tool.name, description: tool.description, parameters: tool.input_schema })),
        }),
      });
      if (!response.ok) throw new Error(`Responses request failed (${response.status})`);
      if (!response.body) throw new Error("Responses stream did not include a body");
      let completed = false;
      for await (const event of sse(response.body)) {
        const data = parseEvent(event.data);
        if (event.event === "response.output_text.delta" || event.event === "response.text.delta") {
          const delta = typeof data.delta === "string" ? data.delta : "";
          if (delta) notify("text_delta", { request_id: requestId, delta });
        } else if (event.event === "response.output_item.done") {
          const item = asRecord(data.item);
          if (item?.type === "function_call") {
            const argumentsValue = parseArguments(item.arguments);
            const id = pickString(item, ["call_id", "id"]);
            const name = pickString(item, ["name"]);
            if (id && name) notify("tool_call", { request_id: requestId, id, name, arguments: argumentsValue });
          }
        } else if (event.event === "response.completed") {
          notify("completed", { request_id: requestId, metadata: data });
          completed = true;
        } else if (event.event === "response.failed" || event.event === "error") {
          throw new Error(pickString(data, ["message", "error"]) ?? "Responses stream failed");
        }
      }
      if (!completed) notify("completed", { request_id: requestId });
      return { metadata: { completed } };
    } catch (error) {
      if (signal?.aborted) return { metadata: { completed: false, cancelled: true } };
      const message = error instanceof Error ? error.message : String(error);
      notify("failed", { request_id: requestId, message });
      throw error;
    }
  }

  private async token(parameters: Record<string, string>): Promise<Credentials> {
    const body = new URLSearchParams({ ...parameters, client_id: this.config.clientId });
    const response = await fetch(new URL("oauth/token", withSlash(this.config.issuer)), {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body,
    });
    if (!response.ok) throw new Error(`OAuth token request failed (${response.status})`);
    const token = await response.json() as Credentials & { expires_in?: number };
    if (!token.access_token) throw new Error("OAuth token response did not include an access token");
    const accountId = token.chatgpt_account_id ?? accountIdFromIdToken(token.id_token);
    return {
      ...token,
      ...(accountId ? { chatgpt_account_id: accountId } : {}),
      ...(typeof token.expires_in === "number" ? { expires_at: Date.now() + token.expires_in * 1000 } : {}),
    };
  }
}

function withSlash(url: string) { return url.endsWith("/") ? url : `${url}/`; }
function bindCallbackServer(fetch: (request: Request) => Response): ReturnType<typeof Bun.serve> {
  try {
    return Bun.serve({ hostname: "127.0.0.1", port: 1455, fetch });
  } catch (error) {
    if (!isAddressInUse(error)) throw error;
    return Bun.serve({ hostname: "127.0.0.1", port: 1457, fetch });
  }
}
function isAddressInUse(error: unknown): boolean {
  return error instanceof Error && "code" in error && error.code === "EADDRINUSE";
}
function authHeaders(credentials: Credentials): Record<string, string> {
  const headers: Record<string, string> = { authorization: `Bearer ${credentials.access_token}` };
  if (credentials.chatgpt_account_id && /^[\x20-\x7e]+$/.test(credentials.chatgpt_account_id)) {
    headers["ChatGPT-Account-ID"] = credentials.chatgpt_account_id;
  }
  return headers;
}
function accountIdFromIdToken(idToken: Json | undefined): string | undefined {
  if (typeof idToken !== "string") return undefined;
  const payload = idToken.split(".")[1];
  if (!payload) return undefined;
  try {
    const claims = asRecord(JSON.parse(Buffer.from(payload, "base64url").toString("utf8")));
    const openAiClaims = asRecord(claims?.["https://api.openai.com/auth"]);
    const accountId = openAiClaims?.chatgpt_account_id;
    return typeof accountId === "string" && accountId.length > 0 ? accountId : undefined;
  } catch {
    return undefined;
  }
}
async function responseError(prefix: string, response: Response): Promise<string> {
  try {
    const payload = asRecord(JSON.parse(await response.text()));
    const nested = asRecord(payload?.error);
    const message = pickString(nested ?? payload ?? {}, ["message"]);
    if (message) return `${prefix} (${response.status}): ${safeErrorDetail(message)}`;
  } catch {
    // Only structured JSON messages are surfaced; arbitrary bodies may contain secrets.
  }
  return `${prefix} (${response.status})`;
}
function safeErrorDetail(message: string): string {
  return message
    .replace(/Bearer\s+\S+/gi, "Bearer <redacted>")
    .replace(/\b(?:sk-[A-Za-z0-9_-]+|eyJ[A-Za-z0-9_-]*\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+)\b/g, "<redacted>")
    .replace(/[\u0000-\u001f\u007f]/g, " ")
    .trim()
    .slice(0, 512);
}
function positiveInteger(value: string | undefined, fallback: number): number {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : fallback;
}
function randomUrlToken(bytes: number) { return Buffer.from(crypto.getRandomValues(new Uint8Array(bytes))).toString("base64url"); }
async function pkceChallenge(verifier: string) { return Buffer.from(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier))).toString("base64url"); }
function asRecord(value: unknown): Record<string, unknown> | undefined { return value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : undefined; }
function pickString(value: Record<string, unknown>, names: string[]) { const candidate = names.map((name) => value[name]).find((item) => typeof item === "string"); return typeof candidate === "string" ? candidate : undefined; }
function pickNumber(value: Record<string, unknown>, names: string[]) { const candidate = names.map((name) => value[name]).find((item) => typeof item === "number"); return typeof candidate === "number" ? candidate : undefined; }
function parseEvent(value: string): Record<string, unknown> { try { return asRecord(JSON.parse(value)) ?? {}; } catch { return {}; } }
function parseArguments(value: unknown): Json { if (typeof value !== "string") return (value ?? {}) as Json; try { return JSON.parse(value) as Json; } catch { return { raw: value }; } }

function responseInput(messages: Array<Record<string, unknown>>): Json[] {
  const input: Json[] = [];
  for (const message of messages) {
    if (message.role === "assistant" && Array.isArray(message.tool_calls)) {
      for (const call of message.tool_calls) {
        const tool = call as Record<string, unknown>;
        input.push({
          type: "function_call",
          call_id: String(tool.id ?? ""),
          name: String(tool.name ?? ""),
          arguments: JSON.stringify(tool.arguments ?? {}),
        });
      }
    }
    if (message.role === "tool" && Array.isArray(message.tool_results)) {
      for (const result of message.tool_results) {
        const tool = result as Record<string, unknown>;
        input.push({ type: "function_call_output", call_id: String(tool.tool_call_id ?? ""), output: String(tool.content ?? "") });
      }
      continue;
    }
    input.push({ role: String(message.role ?? "user"), content: String(message.content ?? "") });
  }
  return input;
}

async function* sse(stream: ReadableStream<Uint8Array>): AsyncGenerator<{ event: string; data: string }> {
  const reader = stream.getReader();
  const decoder = new TextDecoder();
  let pending = "";
  try {
    while (true) {
      const { value, done } = await reader.read();
      pending += decoder.decode(value, { stream: !done });
      let separator: RegExpExecArray | null;
      while ((separator = /\r\n\r\n|\n\n|\r\r/.exec(pending)) !== null) {
        const block = pending.slice(0, separator.index);
        pending = pending.slice(separator.index + separator[0].length);
        const lines = block.split(/\r\n|\n|\r/);
        const event = lines.find((line) => line.startsWith("event:"))?.slice(6).trimStart() ?? "message";
        const data = lines
          .filter((line) => line.startsWith("data:"))
          .map((line) => line.slice(5).trimStart())
          .join("\n");
        if (data !== "[DONE]") yield { event, data };
      }
      if (done) break;
    }
  } finally { reader.releaseLock(); }
}
