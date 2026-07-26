/** Anthropic subscription usage retrieval and normalization. */
import { fetchWithTimeout } from "./auth";
import type { ProviderConfig } from "./config";
import type { Credentials } from "./types";

type UsageLimit = {
	id: string;
	label: string;
	amount: { used: number; limit: number; remaining: number; unit: "percent" };
	window?: { duration_ms?: number; resets_at?: number };
	status: "ok" | "warning" | "exhausted";
};

/** HTTP failure from the Claude usage endpoint. */
export class UsageRequestError extends Error {
	/** Creates a typed failure so the provider can refresh once after a 401. */
	constructor(readonly status: number) {
		super(`Anthropic usage request failed (${status})`);
	}
}

const WINDOWS = [
	["five_hour", "five-hour", "5 hour limit", 5 * 60 * 60 * 1000],
	["seven_day", "seven-day", "7 day limit", 7 * 24 * 60 * 60 * 1000],
	["seven_day_opus", "seven-day-opus", "7 day Opus limit", 7 * 24 * 60 * 60 * 1000],
	["seven_day_sonnet", "seven-day-sonnet", "7 day Sonnet limit", 7 * 24 * 60 * 60 * 1000],
] as const;

/** Fetches Claude account limits and translates the OAuth usage response to capability version 1. */
export async function fetchUsage(
	config: ProviderConfig,
	credentials: Credentials,
	signal?: AbortSignal,
): Promise<{ fetched_at: number; limits: UsageLimit[] }> {
	const response = await fetchWithTimeout(
		usageUrl(config.apiBaseUrl),
		{
			headers: {
				authorization: `Bearer ${credentials["access_token"]}`,
				accept: "application/json",
				"anthropic-beta": "claude-code-20250219,oauth-2025-04-20",
				"user-agent": "claude-cli/2.1.165",
			},
			signal,
		},
		config.requestTimeoutMs,
	);
	if (!response.ok) throw new UsageRequestError(response.status);
	const payload: unknown = await response.json();
	if (!isRecord(payload)) throw new Error("Anthropic usage response must be an object");
	const limits = WINDOWS.flatMap(([field, id, label, duration]) => {
		const bucket = payload[field];
		if (!isRecord(bucket) || !finite(bucket["utilization"])) return [];
		const used = bucket["utilization"];
		return [
			{
				id,
				label,
				amount: { used, limit: 100, remaining: Math.max(0, 100 - used), unit: "percent" as const },
				window: { duration_ms: duration, resets_at: isoTimestamp(bucket["resets_at"]) },
				status:
					used >= 100
						? ("exhausted" as const)
						: used >= 90
							? ("warning" as const)
							: ("ok" as const),
			},
		];
	});
	return { fetched_at: Date.now(), limits };
}

function usageUrl(base: string): URL {
	const url = new URL(base);
	let path = url.pathname.replace(/\/+$/, "");
	if (path.endsWith("/api/oauth")) path = `${path}/usage`;
	else path = `${path.replace(/\/v1$/, "")}/api/oauth/usage`;
	url.pathname = path.replace(/\/+/g, "/");
	return url;
}

function isoTimestamp(value: unknown): number | undefined {
	if (typeof value !== "string") return undefined;
	const parsed = Date.parse(value);
	return Number.isFinite(parsed) ? parsed : undefined;
}

function finite(value: unknown): value is number {
	return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}
