/** Endpoint and timeout configuration, with test-only environment overrides. */

/** Runtime configuration for the OpenAI provider adapter. */
export type ProviderConfig = {
	issuer: string;
	clientId: string;
	codexBaseUrl: string;
	scopes: readonly string[];
	originator: string;
	clientVersion: string;
	authTimeoutMs: number;
	requestTimeoutMs: number;
};

/** Production defaults for the registered ChatGPT OAuth client. */
export const DEFAULT_CONFIG: ProviderConfig = {
	issuer: process.env["MISY_OPENAI_AUTH_ISSUER"] ?? "https://auth.openai.com",
	clientId: process.env["MISY_OPENAI_CLIENT_ID"] ?? "app_EMoamEEZ73f0CkXaXp7hrann",
	codexBaseUrl: process.env["MISY_OPENAI_BASE_URL"] ?? "https://chatgpt.com/backend-api",
	scopes: (
		process.env["MISY_OPENAI_OAUTH_SCOPES"] ??
		"openid profile email offline_access api.connectors.read api.connectors.invoke"
	)
		.split(" ")
		.filter(Boolean),
	originator: process.env["MISY_OPENAI_ORIGINATOR"] ?? "codex_cli_rs",
	clientVersion: process.env["MISY_OPENAI_CLIENT_VERSION"] ?? "0.144.1",
	authTimeoutMs: positiveInteger(process.env["MISY_OPENAI_AUTH_TIMEOUT_MS"], 300_000),
	requestTimeoutMs: positiveInteger(process.env["MISY_OPENAI_REQUEST_TIMEOUT_MS"], 30_000),
};

function positiveInteger(value: string | undefined, fallback: number): number {
	const parsed = Number(value);
	return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : fallback;
}
