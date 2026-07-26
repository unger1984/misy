/** Browser PKCE OAuth lifecycle and token refresh for Claude subscriptions. */
import type { ProviderConfig } from "./config";
import { postOAuthJson } from "./oauth-http";
import { type Credentials, isRecord, type Json } from "./types";

const CALLBACK_URL = "http://localhost:54545/callback";
const EXPIRY_SAFETY_MARGIN_MS = 300_000;

type PendingSession = {
	state: string;
	verifier: string;
	code: Promise<string>;
	expiresAt: number;
	stop: () => void;
	timer: ReturnType<typeof setTimeout>;
};

/** Result returned to the host when browser authorization begins. */
export type AuthSession = {
	url: string;
	session: { id: string };
};

/** Owns only short-lived OAuth sessions; persisted credentials remain core-owned. */
export class OAuthClient {
	private readonly pending = new Map<string, PendingSession>();

	/** Creates an OAuth client using the supplied endpoint configuration. */
	constructor(private readonly config: ProviderConfig) {}

	/** Starts a browser flow or throws if the fixed callback port cannot be reserved. */
	async start(): Promise<AuthSession> {
		const state = randomToken(32);
		const verifier = randomToken(64);
		let resolveCode: (code: string) => void = () => undefined;
		const code = new Promise<string>((resolve) => {
			resolveCode = resolve;
		});
		const server = this.startCallback(state, resolveCode);
		const id = randomToken(18);
		const expiresAt = Date.now() + this.config.authTimeoutMs;
		const timer = setTimeout(() => this.dispose(id), this.config.authTimeoutMs);
		this.pending.set(id, {
			state,
			verifier,
			code,
			expiresAt,
			stop: () => server.stop(true),
			timer,
		});
		const url = new URL(this.config.authorizeUrl);
		url.search = new URLSearchParams({
			code: "true",
			client_id: this.config.clientId,
			response_type: "code",
			redirect_uri: CALLBACK_URL,
			scope: this.config.scopes,
			state,
			code_challenge: await pkceChallenge(verifier),
			code_challenge_method: "S256",
		}).toString();
		return { url: url.toString(), session: { id } };
	}

	/** Exchanges the callback code, always releasing the listener afterwards. */
	async complete(session: unknown, completion: Record<string, unknown>): Promise<Credentials> {
		const pending = this.session(session);
		try {
			const code = suppliedCode(completion) ?? (await waitForCode(pending.code, pending.expiresAt));
			const exchange = splitCodeAndState(code, pending.state);
			return await this.requestToken({
				grant_type: "authorization_code",
				client_id: this.config.clientId,
				code: exchange.code,
				state: exchange.state,
				redirect_uri: CALLBACK_URL,
				code_verifier: pending.verifier,
			});
		} finally {
			this.disposeId(session);
		}
	}

	/** Refreshes an OAuth credential, preserving any fields an endpoint omits. */
	async refresh(credentials: Credentials): Promise<Credentials> {
		const refreshToken = credentials["refresh_token"];
		if (typeof refreshToken !== "string" || refreshToken.length === 0) {
			throw new Error("Anthropic OAuth credentials do not contain a refresh token");
		}
		const refreshed = await this.requestToken(
			{
				grant_type: "refresh_token",
				client_id: this.config.clientId,
				refresh_token: refreshToken,
			},
			{
				"anthropic-beta": "oauth-2025-04-20",
				"user-agent": "anthropic-sdk-typescript/0.94.0 userOAuthProvider",
			},
		);
		return { ...credentials, ...refreshed };
	}

	/** Reports locally-known credential status without contacting Anthropic. */
	status(credentials: Credentials | undefined): { authenticated: boolean; expires_at?: number } {
		if (credentials === undefined || typeof credentials["access_token"] !== "string") {
			return { authenticated: false };
		}
		const expiresAt = credentials["expires_at"];
		return typeof expiresAt === "number"
			? { authenticated: true, expires_at: expiresAt }
			: { authenticated: true };
	}

	private session(session: unknown): PendingSession {
		const id = sessionId(session);
		const pending = id === undefined ? undefined : this.pending.get(id);
		if (pending === undefined) throw new Error("Anthropic OAuth session is missing or expired");
		return pending;
	}

	private disposeId(session: unknown): void {
		const id = sessionId(session);
		if (id !== undefined) this.dispose(id);
	}

	private dispose(id: string): void {
		const pending = this.pending.get(id);
		if (pending === undefined) return;
		clearTimeout(pending.timer);
		pending.stop();
		this.pending.delete(id);
	}

	private startCallback(
		state: string,
		resolveCode: (code: string) => void,
	): ReturnType<typeof Bun.serve> {
		try {
			return Bun.serve({
				hostname: "127.0.0.1",
				port: 54545,
				fetch: (request) => callbackResponse(request, state, resolveCode),
			});
		} catch (cause) {
			if (isPortInUse(cause)) {
				throw new Error(
					"Anthropic OAuth callback port 54545 is already in use; close the other authorization",
				);
			}
			throw new Error("Could not start Anthropic OAuth callback server", { cause });
		}
	}

	private async requestToken(
		fields: Record<string, string>,
		extraHeaders: Record<string, string> = {},
	): Promise<Credentials> {
		const response = await postOAuthJson(
			endpoint(this.config.apiBaseUrl, "v1/oauth/token"),
			{ "content-type": "application/json", ...extraHeaders },
			JSON.stringify(fields),
			this.config.requestTimeoutMs,
		);
		if (response.status < 200 || response.status >= 300)
			throw tokenError(response.status, response.body);
		return credentialsFromToken(response.body);
	}
}

/** Refreshes a token before its five-minute safety margin. */
export async function refreshIfNeeded(
	client: OAuthClient,
	credentials: Credentials,
): Promise<Credentials> {
	const expiresAt = credentials["expires_at"];
	return typeof expiresAt === "number" && expiresAt <= Date.now() + EXPIRY_SAFETY_MARGIN_MS
		? await client.refresh(credentials)
		: credentials;
}

/** Applies both a request deadline and a caller-provided cancellation signal. */
export async function fetchWithTimeout(
	input: URL | string,
	init: RequestInit,
	timeoutMs: number,
): Promise<Response> {
	const deadline = AbortSignal.timeout(timeoutMs);
	const signal =
		init.signal === undefined || init.signal === null
			? deadline
			: AbortSignal.any([init.signal, deadline]);
	return await fetch(input, { ...init, signal });
}

function callbackResponse(
	request: Request,
	state: string,
	resolveCode: (code: string) => void,
): Response {
	const url = new URL(request.url);
	if (url.pathname !== "/callback" || url.searchParams.get("state") !== state) {
		return new Response("OAuth state did not match", { status: 400 });
	}
	const code = url.searchParams.get("code");
	if (code === null || code.length === 0)
		return new Response("OAuth code is missing", { status: 400 });
	resolveCode(code);
	return new Response("Authentication completed. You may close this window.");
}

function splitCodeAndState(value: string, fallbackState: string): { code: string; state: string } {
	const separator = value.indexOf("#");
	if (separator < 0) return { code: value, state: fallbackState };
	const code = value.slice(0, separator);
	const state = value.slice(separator + 1);
	return { code, state: state.length > 0 ? state : fallbackState };
}

function suppliedCode(completion: Record<string, unknown>): string | undefined {
	const code = completion["code"];
	return typeof code === "string" && code.length > 0 ? code : undefined;
}

function sessionId(session: unknown): string | undefined {
	return isRecord(session) && typeof session["id"] === "string" ? session["id"] : undefined;
}

async function waitForCode(code: Promise<string>, expiresAt: number): Promise<string> {
	const remaining = Math.max(0, expiresAt - Date.now());
	let timer: ReturnType<typeof setTimeout> | undefined;
	try {
		return await Promise.race([
			code,
			new Promise<string>((_resolve, reject) => {
				timer = setTimeout(
					() =>
						reject(new Error("Anthropic OAuth session expired before authentication completed")),
					remaining,
				);
			}),
		]);
	} finally {
		if (timer !== undefined) clearTimeout(timer);
	}
}

function credentialsFromToken(body: string): Credentials {
	const value = parseJson(body, "Anthropic OAuth token response");
	if (!isRecord(value) || typeof value["access_token"] !== "string") {
		throw new Error("Anthropic OAuth token response did not include access_token");
	}
	const expiresIn = value["expires_in"];
	return {
		...value,
		access_token: value["access_token"],
		type: "oauth",
		...(typeof expiresIn === "number"
			? { expires_at: Date.now() + expiresIn * 1_000 - EXPIRY_SAFETY_MARGIN_MS }
			: {}),
	};
}

function tokenError(status: number, body: string): Error {
	const payload = tryJson(body);
	const code = isRecord(payload) ? payload["error"] : undefined;
	if (code === "invalid_grant") {
		return new Error("Anthropic refresh token has expired; sign in to Claude again");
	}
	const detail =
		isRecord(payload) && typeof payload["message"] === "string" ? `: ${payload["message"]}` : "";
	return new Error(`Anthropic OAuth token request failed (${status})${detail}`);
}

function parseJson(body: string, source: string): Json {
	const parsed = tryJson(body);
	if (parsed === undefined) throw new Error(`${source} returned invalid JSON`);
	return parsed;
}

function tryJson(body: string): Json | undefined {
	try {
		return JSON.parse(body) as Json;
	} catch {
		return undefined;
	}
}

function endpoint(baseUrl: string, path: string): URL {
	return new URL(path, baseUrl.endsWith("/") ? baseUrl : `${baseUrl}/`);
}

function randomToken(bytes: number): string {
	return Buffer.from(crypto.getRandomValues(new Uint8Array(bytes))).toString("base64url");
}

async function pkceChallenge(verifier: string): Promise<string> {
	const hash = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
	return Buffer.from(hash).toString("base64url");
}

function isPortInUse(cause: unknown): boolean {
	return cause instanceof Error && "code" in cause && cause.code === "EADDRINUSE";
}
