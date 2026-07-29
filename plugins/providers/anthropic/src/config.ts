/** Configuration values kept at the network edge, with supported user environment overrides. */

/** Runtime endpoint and deadline configuration for the Anthropic provider. */
export type ProviderConfig = {
	authorizeUrl: string;
	apiBaseUrl: string;
	clientId: string;
	scopes: string;
	authTimeoutMs: number;
	requestTimeoutMs: number;
};

/**
 * Production defaults for the Claude subscription OAuth client.
 *
 * Every field is overridable from the plugin process environment. This is a supported
 * user feature — for routing through a proxy or gateway, or for debugging — not a
 * test-only hook: `MISY_ANTHROPIC_AUTHORIZE_URL`, `MISY_ANTHROPIC_API_BASE_URL`,
 * `MISY_ANTHROPIC_CLIENT_ID`, `MISY_ANTHROPIC_OAUTH_SCOPES`,
 * `MISY_ANTHROPIC_AUTH_TIMEOUT_MS`, `MISY_ANTHROPIC_REQUEST_TIMEOUT_MS`.
 */
export const DEFAULT_CONFIG: ProviderConfig = {
	authorizeUrl: process.env["MISY_ANTHROPIC_AUTHORIZE_URL"] ?? "https://claude.ai/oauth/authorize",
	apiBaseUrl: process.env["MISY_ANTHROPIC_API_BASE_URL"] ?? "https://api.anthropic.com",
	clientId: process.env["MISY_ANTHROPIC_CLIENT_ID"] ?? "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
	scopes:
		process.env["MISY_ANTHROPIC_OAUTH_SCOPES"] ??
		"org:create_api_key user:profile user:inference user:sessions:claude_code " +
			"user:mcp_servers user:file_upload",
	authTimeoutMs: positiveInteger(process.env["MISY_ANTHROPIC_AUTH_TIMEOUT_MS"], 300_000),
	requestTimeoutMs: positiveInteger(process.env["MISY_ANTHROPIC_REQUEST_TIMEOUT_MS"], 30_000),
};

function positiveInteger(value: string | undefined, fallback: number): number {
	const parsed = Number(value);
	return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : fallback;
}
