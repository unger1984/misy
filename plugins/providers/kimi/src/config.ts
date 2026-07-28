/** Kimi endpoint and storage configuration, with supported user environment overrides. */
import { homedir } from "node:os";
import { join } from "node:path";

/** Runtime configuration for the Kimi provider adapter. */
export type ProviderConfig = {
	authBaseUrl: string;
	apiBaseUrl: string;
	clientId: string;
	packageVersion: string;
	requestTimeoutMs: number;
	dataDir: string;
};

/**
 * Production defaults for Kimi's registered coding client.
 *
 * Every field except `packageVersion` is overridable from the plugin process environment.
 * This is a supported user feature — for routing through a proxy or gateway, or for
 * debugging — not a test-only hook: `MISY_KIMI_AUTH_BASE_URL`, `MISY_KIMI_API_BASE_URL`,
 * `MISY_KIMI_CLIENT_ID`, `MISY_KIMI_REQUEST_TIMEOUT_MS`, `MISY_KIMI_DATA_DIR`.
 */
export const DEFAULT_CONFIG: ProviderConfig = {
	authBaseUrl: process.env["MISY_KIMI_AUTH_BASE_URL"] ?? "https://auth.kimi.com",
	apiBaseUrl: process.env["MISY_KIMI_API_BASE_URL"] ?? "https://api.kimi.com/coding/v1",
	clientId: process.env["MISY_KIMI_CLIENT_ID"] ?? "17e5f671-d194-4dfb-9706-5516cb48c098",
	packageVersion: "0.1.0",
	requestTimeoutMs: positiveInteger(process.env["MISY_KIMI_REQUEST_TIMEOUT_MS"], 30_000),
	dataDir: process.env["MISY_KIMI_DATA_DIR"] ?? join(homedir(), ".local", "share", "misy", "kimi"),
};

function positiveInteger(value: string | undefined, fallback: number): number {
	const parsed = Number(value);
	return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : fallback;
}
