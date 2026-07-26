/** OAuth lifecycle and account identity for the OpenAI provider. */
import type { ProviderConfig } from "./config";
import type { Credentials, Json } from "./types";

type Pending = {
	verifier: string;
	code: Promise<string>;
	expiresAt: number;
	stop: () => void;
};
export type AuthSession = {
	id: string;
	url: string;
	session: { id: string };
};

/** Owns temporary browser OAuth sessions. */
export class OAuthClient {
	private readonly pending = new Map<string, Pending>();
	constructor(private readonly config: ProviderConfig) {}

	/** Opens a fixed registered callback listener or reports that it is occupied. */
	async start(): Promise<AuthSession> {
		const state = token(32);
		const verifier = token(64);
		let resolve!: (code: string) => void;
		const code = new Promise<string>((done) => {
			resolve = done;
		});
		const server = this.callback(state, resolve);
		const id = token(18);
		const stop = () => server.stop(true);
		const expiresAt = Date.now() + this.config.authTimeoutMs;
		this.pending.set(id, { verifier, code, expiresAt, stop });
		setTimeout(() => {
			const pending = this.pending.get(id);
			if (pending) {
				pending.stop();
				this.pending.delete(id);
			}
		}, this.config.authTimeoutMs);
		const url = new URL("/oauth/authorize", this.config.issuer);
		url.search = new URLSearchParams({
			response_type: "code",
			client_id: this.config.clientId,
			redirect_uri: "http://localhost:1455/auth/callback",
			scope: this.config.scopes.join(" "),
			state,
			code_challenge: await challenge(verifier),
			code_challenge_method: "S256",
			id_token_add_organizations: "true",
			codex_cli_simplified_flow: "true",
			originator: this.config.originator,
		}).toString();
		return { id, url: url.toString(), session: { id } };
	}

	/** Exchanges a callback code and releases its listener on every outcome. */
	async complete(session: unknown, completion: Record<string, unknown>): Promise<Credentials> {
		const id = isRecord(session) && typeof session["id"] === "string" ? session["id"] : "";
		const pending = this.pending.get(id);
		if (!pending) throw new Error("OAuth session is missing or expired");
		try {
			const supplied = typeof completion["code"] === "string" ? completion["code"] : undefined;
			const code = supplied ?? (await waitForCode(pending.code, pending.expiresAt));
			return await this.requestToken({
				grant_type: "authorization_code",
				code,
				code_verifier: pending.verifier,
				redirect_uri: "http://localhost:1455/auth/callback",
			});
		} finally {
			pending.stop();
			this.pending.delete(id);
		}
	}

	/** Refreshes an OAuth token while preserving fields an endpoint omits. */
	async refresh(credentials: Credentials): Promise<Credentials> {
		const refresh = credentials["refresh_token"];
		if (!refresh) throw new Error("OpenAI OAuth credentials do not contain a refresh token");
		return {
			...credentials,
			...(await this.requestToken(
				{ grant_type: "refresh_token", refresh_token: refresh },
				credentials["chatgpt_account_id"],
			)),
		};
	}

	/** Reports status without a network call. */
	status(credentials: Credentials | undefined): { authenticated: boolean; expires_at?: number } {
		if (!credentials?.["access_token"]) return { authenticated: false };
		return credentials["expires_at"] === undefined
			? { authenticated: true }
			: { authenticated: true, expires_at: credentials["expires_at"] };
	}

	private callback(state: string, resolve: (code: string) => void): ReturnType<typeof Bun.serve> {
		try {
			return Bun.serve({
				hostname: "127.0.0.1",
				port: 1455,
				fetch: (request) => {
					const url = new URL(request.url);
					if (url.pathname !== "/auth/callback" || url.searchParams.get("state") !== state) {
						return new Response("OAuth state did not match", { status: 400 });
					}
					const code = url.searchParams.get("code");
					if (!code) return new Response("OAuth code is missing", { status: 400 });
					resolve(code);
					return new Response("Authentication completed. You may close this window.");
				},
			});
		} catch (cause) {
			if (cause instanceof Error && "code" in cause && cause.code === "EADDRINUSE") {
				throw new Error(
					"OpenAI OAuth callback port 1455 is already in use; close the other authorization",
				);
			}
			throw cause;
		}
	}

	private async requestToken(
		fields: Record<string, string>,
		accountFallback?: string,
	): Promise<Credentials> {
		const response = await fetchWithTimeout(
			new URL("oauth/token", slash(this.config.issuer)),
			{
				method: "POST",
				headers: { "content-type": "application/x-www-form-urlencoded" },
				body: new URLSearchParams({ ...fields, client_id: this.config.clientId }),
			},
			this.config.requestTimeoutMs,
		);
		if (!response.ok) throw new Error(`OpenAI OAuth token request failed (${response.status})`);
		const value: unknown = await response.json();
		if (!isRecord(value) || typeof value["access_token"] !== "string") {
			throw new Error("OpenAI OAuth token response did not include access_token");
		}
		const account =
			typeof value["chatgpt_account_id"] === "string"
				? value["chatgpt_account_id"]
				: (accountId(value["id_token"]) ?? accountFallback);
		if (!account) throw new Error("OpenAI OAuth token did not contain a ChatGPT account id");
		return {
			...value,
			access_token: value["access_token"],
			chatgpt_account_id: account,
			type: "oauth",
			...(typeof value["expires_in"] === "number"
				? { expires_at: Date.now() + value["expires_in"] * 1000 }
				: {}),
		} as Credentials;
	}
}

async function waitForCode(code: Promise<string>, expiresAt: number): Promise<string> {
	const remaining = Math.max(0, expiresAt - Date.now());
	let timeout: ReturnType<typeof setTimeout> | undefined;
	try {
		return await Promise.race([
			code,
			new Promise<string>((_resolve, reject) => {
				timeout = setTimeout(
					() => reject(new Error("OAuth session expired before authentication completed")),
					remaining,
				);
			}),
		]);
	} finally {
		if (timeout !== undefined) clearTimeout(timeout);
	}
}

/** Refreshes credentials before their one-minute safety margin. */
export async function refreshIfNeeded(
	client: OAuthClient,
	credentials: Credentials,
): Promise<Credentials> {
	return credentials["expires_at"] !== undefined && credentials["expires_at"] <= Date.now() + 60_000
		? await client.refresh(credentials)
		: credentials;
}

/** Produces account-scoped authorization headers. */
export function authHeaders(credentials: Credentials): Record<string, string> {
	const account = credentials["chatgpt_account_id"];
	if (!account)
		throw new Error("OpenAI OAuth credentials do not contain a valid ChatGPT account id");
	return { authorization: `Bearer ${credentials["access_token"]}`, "chatgpt-account-id": account };
}

/** Applies a deadline to all provider HTTP requests. */
export async function fetchWithTimeout(
	input: URL | string,
	init: RequestInit,
	timeout: number,
): Promise<Response> {
	const deadline = AbortSignal.timeout(timeout);
	const signal = init.signal ? AbortSignal.any([init.signal, deadline]) : deadline;
	return await fetch(input, { ...init, signal });
}

function accountId(idToken: Json | undefined): string | undefined {
	if (typeof idToken !== "string") return undefined;
	try {
		const payload: unknown = JSON.parse(
			Buffer.from(idToken.split(".")[1] ?? "", "base64url").toString(),
		);
		const auth = isRecord(payload) ? payload["https://api.openai.com/auth"] : undefined;
		return isRecord(auth) && typeof auth["chatgpt_account_id"] === "string"
			? auth["chatgpt_account_id"]
			: undefined;
	} catch {
		return undefined;
	}
}
function token(bytes: number): string {
	return Buffer.from(crypto.getRandomValues(new Uint8Array(bytes))).toString("base64url");
}
async function challenge(value: string): Promise<string> {
	return Buffer.from(
		await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value)),
	).toString("base64url");
}
function slash(value: string): string {
	return value.endsWith("/") ? value : `${value}/`;
}
function isRecord(value: unknown): value is Record<string, Json> {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}
