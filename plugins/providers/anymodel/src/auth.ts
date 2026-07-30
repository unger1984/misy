/** Prompt API-key authentication primitives kept separate from remote catalog validation. */
import type { ApiKeyCredentials, Json } from "./types";

const SESSION = { method: "api_key" };

/** Starts the one supported AnyModel prompt flow. */
export function startAuth(method: string): Record<string, Json> {
	if (method !== "api_key")
		throw new Error(`Unsupported AnyModel authentication method: ${method}`);
	return {
		kind: "prompt",
		fields: [{ id: "api_key", label: "API key", secret: true }],
		session: SESSION,
	};
}

/** Validates the prompt-session marker and returns a trimmed opaque API-key credential. */
export function completeAuth(
	session: unknown,
	completion: Record<string, Json>,
): ApiKeyCredentials {
	if (!isSession(session)) throw new Error("Invalid AnyModel authentication session");
	const value = completion["api_key"];
	if (typeof value !== "string" || value.trim().length === 0)
		throw new Error("AnyModel API key is required");
	return { type: "api_key", api_key: value.trim() };
}

/** Reports local credential validity without exposing or remotely checking the key. */
export function authStatus(credentials: ApiKeyCredentials | undefined): { authenticated: boolean } {
	return { authenticated: credentials !== undefined && credentials.api_key.trim().length !== 0 };
}

function isSession(value: unknown): boolean {
	return (
		value !== null &&
		typeof value === "object" &&
		(value as { method?: unknown }).method === "api_key"
	);
}
