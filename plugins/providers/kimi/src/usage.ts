/** Kimi subscription usage retrieval and normalization. */
import type { ProviderConfig } from "./config";
import type { KimiHeaders } from "./headers";
import { fetchWithTimeout } from "./http";
import type { Credentials } from "./types";

type UsageLimit = {
	id: string;
	label: string;
	amount: { used?: number; limit?: number; remaining?: number; unit: "unknown" };
	window?: { duration_ms?: number; resets_at?: number };
	status: "ok" | "warning" | "exhausted" | "unknown";
};

/** HTTP failure from the Kimi usage endpoint. */
export class UsageRequestError extends Error {
	/** Creates a typed failure so the provider can refresh once after a 401. */
	constructor(readonly status: number) {
		super(`Kimi usage request failed (${status})`);
	}
}

/** Fetches Kimi Coding quotas and translates its usage response to capability version 1. */
export async function fetchUsage(
	config: ProviderConfig,
	headers: KimiHeaders,
	credentials: Credentials,
	signal?: AbortSignal,
): Promise<{ fetched_at: number; limits: UsageLimit[] }> {
	const response = await fetchWithTimeout(
		endpoint(config.apiBaseUrl, "usages"),
		{
			headers: { ...headers.common(), authorization: `Bearer ${credentials.access_token}` },
			signal,
		},
		config.requestTimeoutMs,
	);
	if (!response.ok) throw new UsageRequestError(response.status);
	const payload: unknown = await response.json();
	if (!isRecord(payload)) throw new Error("Kimi usage response must be an object");
	const now = Date.now();
	const limits: UsageLimit[] = [];
	if (isRecord(payload["usage"])) addLimit(limits, payload["usage"], "total", "Total quota", now);
	if (Array.isArray(payload["limits"])) {
		for (const [index, item] of payload["limits"].entries()) {
			if (!isRecord(item)) continue;
			const detail = isRecord(item["detail"]) ? item["detail"] : item;
			const label =
				text(item["name"]) ??
				text(item["title"]) ??
				text(item["scope"]) ??
				windowLabel(item["window"]) ??
				`Limit ${index + 1}`;
			addLimit(limits, detail, `limit-${index}`, label, now, item["window"]);
		}
	}
	return { fetched_at: now, limits };
}

function addLimit(
	limits: UsageLimit[],
	data: Record<string, unknown>,
	id: string,
	label: string,
	now: number,
	windowValue?: unknown,
): void {
	const limit = number(data["limit"]);
	const remaining = number(data["remaining"]);
	const used =
		number(data["used"]) ??
		(limit !== undefined && remaining !== undefined ? Math.max(0, limit - remaining) : undefined);
	if (used === undefined && remaining === undefined) return;
	const fraction =
		used !== undefined && limit !== undefined && limit > 0 ? used / limit : undefined;
	const window = isRecord(windowValue) ? windowValue : {};
	limits.push({
		id,
		label,
		amount: { used, limit, remaining, unit: "unknown" },
		window: buildWindow(window, data, now),
		status:
			fraction === undefined
				? "unknown"
				: fraction >= 1
					? "exhausted"
					: fraction >= 0.9
						? "warning"
						: "ok",
	});
}

/** Derives a display label from the window when the limit entry carries no name, as upstream does. */
function windowLabel(value: unknown): string | undefined {
	if (!isRecord(value)) return undefined;
	const duration = number(value["duration"]);
	const unit = text(value["timeUnit"])?.toUpperCase();
	if (duration === undefined || unit === undefined) return undefined;
	if (unit.includes("MINUTE"))
		return duration % 60 === 0 ? `${duration / 60}h limit` : `${duration}m limit`;
	if (unit.includes("HOUR")) return `${duration}h limit`;
	if (unit.includes("DAY")) return `${duration}d limit`;
	return undefined;
}

function buildWindow(
	window: Record<string, unknown>,
	detail: Record<string, unknown>,
	now: number,
): UsageLimit["window"] {
	const duration = number(window["duration"]);
	const unit = text(window["timeUnit"])?.toUpperCase();
	const multiplier = unit?.includes("DAY")
		? 86_400_000
		: unit?.includes("HOUR")
			? 3_600_000
			: unit?.includes("MINUTE")
				? 60_000
				: undefined;
	const resetsAt = resetTime(detail, now) ?? resetTime(window, now);
	return duration !== undefined || resetsAt !== undefined
		? {
				duration_ms:
					duration !== undefined && multiplier !== undefined ? duration * multiplier : undefined,
				resets_at: resetsAt,
			}
		: undefined;
}

function resetTime(value: Record<string, unknown>, now: number): number | undefined {
	for (const key of ["reset_at", "resetAt", "reset_time", "resetTime"]) {
		const raw = value[key];
		if (typeof raw === "string") {
			const parsed = Date.parse(raw);
			if (Number.isFinite(parsed)) return parsed;
		}
		const numeric = number(raw);
		if (numeric !== undefined) return numeric < 1_000_000_000_000 ? numeric * 1000 : numeric;
	}
	const seconds = number(value["reset_in"]) ?? number(value["resetIn"]) ?? number(value["ttl"]);
	return seconds === undefined ? undefined : now + seconds * 1000;
}

function number(value: unknown): number | undefined {
	// Kimi's /usages endpoint serializes quantities as strings ("100"), so accept both forms.
	if (typeof value === "string") {
		const trimmed = value.trim();
		if (!trimmed) return undefined;
		value = Number(trimmed);
	}
	return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : undefined;
}

function text(value: unknown): string | undefined {
	return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function endpoint(base: string, path: string): URL {
	return new URL(path, base.endsWith("/") ? base : `${base}/`);
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}
