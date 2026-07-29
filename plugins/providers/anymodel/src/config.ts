/** AnyModel endpoint configuration, including supported process-level overrides. */

/** Runtime configuration for the AnyModel adapter. */
export type ProviderConfig = { baseUrl: string; requestTimeoutMs: number };

/** Production defaults, with overrides useful for gateways and local fake servers. */
export const DEFAULT_CONFIG: ProviderConfig = {
	baseUrl: process.env["MISY_ANYMODEL_BASE_URL"] ?? "https://anymodel.org/v1",
	requestTimeoutMs: positiveInteger(process.env["MISY_ANYMODEL_REQUEST_TIMEOUT_MS"], 30_000),
};

function positiveInteger(value: string | undefined, fallback: number): number {
	const parsed = Number(value);
	return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : fallback;
}
