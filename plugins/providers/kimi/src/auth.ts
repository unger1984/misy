/** Kimi OAuth device authorization lifecycle and token refresh. */
import { randomUUID } from "node:crypto";
import { setTimeout as sleep } from "node:timers/promises";
import { endpointUrl, fetchWithTimeout } from "@misy/provider-sdk";
import type { ProviderConfig } from "./config";
import type { KimiHeaders } from "./headers";
import { type Credentials, isRecord } from "./types";

const DEVICE_GRANT = "urn:ietf:params:oauth:grant-type:device_code";
const DEFAULT_DEVICE_TTL_MS = 15 * 60 * 1000;
const DEFAULT_POLL_INTERVAL_MS = 5_000;
const EXPIRY_SKEW_MS = 5 * 60 * 1_000;

type PendingDevice = {
	deviceCode: string;
	deadline: number;
	intervalMs: number;
	controller: AbortController;
	expiryTimer: ReturnType<typeof setTimeout>;
};

/** Owns temporary Kimi device authorization sessions. */
export class OAuthClient {
	private readonly pending = new Map<string, PendingDevice>();

	/** Creates an OAuth client bound to Kimi endpoints and common headers. */
	constructor(
		private readonly config: ProviderConfig,
		private readonly headers: KimiHeaders,
	) {}

	/** Starts Kimi's device flow and returns its v2 protocol response. */
	async start(method = "oauth"): Promise<Record<string, unknown>> {
		if (method !== "oauth") throw new Error(`Unsupported Kimi authentication method: ${method}`);
		const authorization = await this.requestDeviceAuthorization();
		const id = randomUUID();
		const controller = new AbortController();
		const expiryTimer = setTimeout(() => this.cancel(id), authorization.expiresInMs);
		this.pending.set(id, {
			deviceCode: authorization.deviceCode,
			deadline: Date.now() + authorization.expiresInMs,
			intervalMs: authorization.intervalMs,
			controller,
			expiryTimer,
		});
		return {
			kind: "device",
			url: authorization.url,
			user_code: authorization.userCode,
			expires_at: Date.now() + authorization.expiresInMs,
			session: { id },
		};
	}

	/** Polls an existing device session until Kimi issues credentials or it expires. */
	async complete(session: unknown): Promise<Credentials> {
		const id = sessionId(session);
		const pending = this.pending.get(id);
		if (!pending) throw new Error("Kimi device authorization session is missing or expired");
		try {
			return await this.pollForToken(pending);
		} finally {
			this.remove(id);
		}
	}

	/** Refreshes credentials, retaining the prior refresh token when Kimi omits a replacement. */
	async refresh(credentials: Credentials): Promise<Credentials> {
		if (!credentials.refresh_token)
			throw new Error("Kimi OAuth credentials do not contain a refresh token");
		return await this.requestToken(
			{
				grant_type: "refresh_token",
				refresh_token: credentials.refresh_token,
				client_id: this.config.clientId,
			},
			credentials.refresh_token,
		);
	}

	/** Reports locally-known credential status without making a network request. */
	status(credentials: Credentials | undefined): { authenticated: boolean; expires_at?: number } {
		if (!credentials?.access_token) return { authenticated: false };
		return credentials.expires_at === undefined
			? { authenticated: true }
			: { authenticated: true, expires_at: credentials.expires_at };
	}

	/** Cancels every still-pending device poll when the plugin process is shutting down. */
	cancelAll(): void {
		for (const id of this.pending.keys()) this.cancel(id);
	}

	private async requestDeviceAuthorization(): Promise<{
		userCode: string;
		deviceCode: string;
		url: string;
		expiresInMs: number;
		intervalMs: number;
	}> {
		const response = await fetchWithTimeout(
			endpointUrl(this.config.authBaseUrl, "api/oauth/device_authorization"),
			{
				method: "POST",
				headers: { ...this.headers.common(), "content-type": "application/x-www-form-urlencoded" },
				body: new URLSearchParams({ client_id: this.config.clientId }),
			},
			this.config.requestTimeoutMs,
		);
		const payload = await responsePayload(response);
		if (!response.ok) throw deviceRequestError(response.status, payload);
		const userCode = stringField(payload, "user_code");
		const deviceCode = stringField(payload, "device_code");
		const verificationUri = stringField(payload, "verification_uri");
		const complete = optionalString(payload, "verification_uri_complete");
		if (!userCode || !deviceCode || !verificationUri) {
			throw new Error("Kimi device authorization response missing required fields");
		}
		return {
			userCode,
			deviceCode,
			url: complete || verificationUri,
			expiresInMs: seconds(payload, "expires_in", DEFAULT_DEVICE_TTL_MS),
			intervalMs: Math.max(1_000, seconds(payload, "interval", DEFAULT_POLL_INTERVAL_MS, 1_000)),
		};
	}

	private async pollForToken(pending: PendingDevice): Promise<Credentials> {
		let intervalMs = pending.intervalMs;
		while (Date.now() < pending.deadline) {
			if (pending.controller.signal.aborted)
				throw new Error("Kimi device authorization was cancelled");
			const response = await fetchWithTimeout(
				endpointUrl(this.config.authBaseUrl, "api/oauth/token"),
				{
					method: "POST",
					signal: pending.controller.signal,
					headers: {
						...this.headers.common(),
						"content-type": "application/x-www-form-urlencoded",
					},
					body: new URLSearchParams({
						client_id: this.config.clientId,
						device_code: pending.deviceCode,
						grant_type: DEVICE_GRANT,
					}),
				},
				this.config.requestTimeoutMs,
			);
			const payload = await responsePayload(response);
			if (response.ok && optionalString(payload, "access_token")) return credentials(payload);
			const error = optionalString(payload, "error");
			if (error === "authorization_pending") {
				await this.waitForPoll(intervalMs, pending);
				continue;
			}
			if (error === "slow_down") {
				intervalMs = Math.max(intervalMs + 5_000, seconds(payload, "interval", 0, 1_000));
				await this.waitForPoll(intervalMs, pending);
				continue;
			}
			if (error === "expired_token") throw new Error("Kimi device authorization expired");
			if (error === "access_denied") throw new Error("Kimi device authorization denied");
			throw tokenRequestError(response.status, payload);
		}
		throw new Error("Kimi device authorization timed out");
	}

	private async requestToken(
		fields: Record<string, string>,
		refreshFallback: string,
	): Promise<Credentials> {
		const response = await fetchWithTimeout(
			endpointUrl(this.config.authBaseUrl, "api/oauth/token"),
			{
				method: "POST",
				headers: { ...this.headers.common(), "content-type": "application/x-www-form-urlencoded" },
				body: new URLSearchParams(fields),
			},
			this.config.requestTimeoutMs,
		);
		const payload = await responsePayload(response);
		if (!response.ok) throw tokenRequestError(response.status, payload);
		return credentials(payload, refreshFallback);
	}

	private cancel(id: string): void {
		const pending = this.pending.get(id);
		if (!pending) return;
		pending.controller.abort();
		this.remove(id);
	}

	private remove(id: string): void {
		const pending = this.pending.get(id);
		if (!pending) return;
		clearTimeout(pending.expiryTimer);
		this.pending.delete(id);
	}

	private async waitForPoll(durationMs: number, pending: PendingDevice): Promise<void> {
		try {
			await wait(durationMs, pending.controller.signal);
		} catch (cause) {
			if (Date.now() >= pending.deadline) return;
			throw cause;
		}
	}
}

/** Refreshes credentials when they are inside Kimi's five-minute safety window. */
export async function refreshIfNeeded(
	client: OAuthClient,
	credentials: Credentials,
): Promise<Credentials> {
	return credentials.expires_at !== undefined &&
		credentials.expires_at <= Date.now() + EXPIRY_SKEW_MS
		? await client.refresh(credentials)
		: credentials;
}

function sessionId(session: unknown): string {
	return isRecord(session) && typeof session["id"] === "string" ? session["id"] : "";
}

async function responsePayload(response: Response): Promise<Record<string, unknown>> {
	try {
		const value: unknown = await response.json();
		return isRecord(value) ? value : {};
	} catch {
		return {};
	}
}

function credentials(payload: Record<string, unknown>, refreshFallback?: string): Credentials {
	const accessToken = optionalString(payload, "access_token");
	const refreshToken = optionalString(payload, "refresh_token") ?? refreshFallback;
	const expiresIn = typeof payload["expires_in"] === "number" ? payload["expires_in"] : undefined;
	if (!accessToken || !refreshToken || expiresIn === undefined) {
		throw new Error("Kimi token response missing required fields");
	}
	return {
		access_token: accessToken,
		refresh_token: refreshToken,
		expires_at: Date.now() + expiresIn * 1_000 - EXPIRY_SKEW_MS,
		type: "oauth",
	};
}

function optionalString(payload: Record<string, unknown>, field: string): string | undefined {
	return typeof payload[field] === "string" ? payload[field] : undefined;
}

function stringField(payload: Record<string, unknown>, field: string): string | undefined {
	const value = optionalString(payload, field);
	return value?.trim() ? value : undefined;
}

function seconds(
	payload: Record<string, unknown>,
	field: string,
	fallback: number,
	multiplier = 1_000,
): number {
	const value = payload[field];
	return typeof value === "number" && Number.isFinite(value) && value > 0
		? value * multiplier
		: fallback;
}

function deviceRequestError(status: number, payload: Record<string, unknown>): Error {
	return new Error(`Kimi device authorization request failed (${status})${errorDetail(payload)}`);
}

function tokenRequestError(status: number, payload: Record<string, unknown>): Error {
	return new Error(`Kimi token request failed (${status})${errorDetail(payload)}`);
}

function errorDetail(payload: Record<string, unknown>): string {
	const detail = optionalString(payload, "error_description") ?? optionalString(payload, "error");
	return detail ? `: ${detail}` : "";
}

async function wait(durationMs: number, signal: AbortSignal): Promise<void> {
	await sleep(durationMs, undefined, { signal });
}
