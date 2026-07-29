/** OpenAI subscription usage retrieval and normalization. */
import { fetchWithTimeout } from "@misy/provider-sdk";
import { authHeaders } from "./auth";
import type { ProviderConfig } from "./config";
import type { Credentials } from "./types";

type UsageLimit = {
	id: string;
	label: string;
	amount: { used?: number; limit?: number; remaining?: number; unit: "percent" };
	window?: { duration_ms?: number; resets_at?: number };
	status?: "ok" | "warning" | "exhausted" | "unknown";
};

/** A normalized usage capability version 1 report. */
export type UsageReport = { fetched_at: number; limits: UsageLimit[] };

/** HTTP failure from the ChatGPT usage endpoint. */
export class UsageRequestError extends Error {
	/** Creates a typed failure so the provider can refresh once after a 401. */
	constructor(readonly status: number) {
		super(`OpenAI usage request failed (${status})`);
	}
}

/** Fetches ChatGPT account limits and translates `/wham/usage` into usage capability version 1. */
export async function fetchUsage(
	config: ProviderConfig,
	credentials: Credentials,
	signal?: AbortSignal,
): Promise<UsageReport> {
	const response = await fetchWithTimeout(
		usageUrl(config.codexBaseUrl),
		{ headers: { ...authHeaders(credentials), accept: "application/json" }, signal },
		config.requestTimeoutMs,
	);
	if (!response.ok) throw new UsageRequestError(response.status);
	const payload: unknown = await response.json();
	if (!isRecord(payload)) throw new Error("OpenAI usage response must be an object");
	const fetchedAt = Date.now();
	const limits: UsageLimit[] = [];
	const rateLimit = isRecord(payload["rate_limit"]) ? payload["rate_limit"] : undefined;
	if (rateLimit) {
		addWindow(limits, "primary", "Primary limit", rateLimit["primary_window"], fetchedAt);
		addWindow(limits, "secondary", "Secondary limit", rateLimit["secondary_window"], fetchedAt);
	}
	const additional = payload["additional_rate_limits"];
	if (Array.isArray(additional)) {
		for (const [index, item] of additional.entries()) {
			if (!isRecord(item) || !isRecord(item["rate_limit"])) continue;
			const name =
				typeof item["limit_name"] === "string" ? item["limit_name"] : `Limit ${index + 1}`;
			addWindow(
				limits,
				`additional-${index}-primary`,
				`${name} primary`,
				item["rate_limit"]["primary_window"],
				fetchedAt,
			);
			addWindow(
				limits,
				`additional-${index}-secondary`,
				`${name} secondary`,
				item["rate_limit"]["secondary_window"],
				fetchedAt,
			);
		}
	}
	return { fetched_at: fetchedAt, limits };
}

function addWindow(
	limits: UsageLimit[],
	id: string,
	label: string,
	value: unknown,
	now: number,
): void {
	if (!isRecord(value) || !finite(value["used_percent"])) return;
	const used = value["used_percent"];
	const durationSeconds = finite(value["limit_window_seconds"])
		? value["limit_window_seconds"]
		: undefined;
	const resetAt = timestamp(value["reset_at"]);
	const resetAfter = finite(value["reset_after_seconds"])
		? now + value["reset_after_seconds"] * 1000
		: undefined;
	limits.push({
		id,
		label,
		amount: { used, limit: 100, remaining: Math.max(0, 100 - used), unit: "percent" },
		window:
			durationSeconds !== undefined || resetAt !== undefined || resetAfter !== undefined
				? {
						duration_ms: durationSeconds === undefined ? undefined : durationSeconds * 1000,
						resets_at: resetAt ?? resetAfter,
					}
				: undefined,
		status: status(used),
	});
}

function status(used: number): UsageLimit["status"] {
	return used >= 100 ? "exhausted" : used >= 90 ? "warning" : "ok";
}

function timestamp(value: unknown): number | undefined {
	return finite(value) ? (value < 1_000_000_000_000 ? value * 1000 : value) : undefined;
}

function finite(value: unknown): value is number {
	return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

function usageUrl(base: string): URL {
	const url = new URL(base);
	let path = url.pathname.replace(/\/+$/, "");
	if (path.endsWith("/codex/responses")) path = path.slice(0, -"/codex/responses".length);
	else if (path.endsWith("/codex")) path = path.slice(0, -"/codex".length);
	url.pathname = `${path}/wham/usage`;
	return url;
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}
