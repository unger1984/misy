/**
 * Validation of the opaque OAuth credentials the core echoes back inside request params.
 *
 * Credentials cross a process boundary, so they are `unknown` JSON until narrowed here. A cast is
 * not validation: only the two fields the protocol relies on are checked, and every other field is
 * preserved untouched for the issuing provider.
 */
import {
	type ApiKeyCredentials,
	isRecord,
	type OAuthCredentials,
	type ProviderCredentials,
} from "./types";

/** Returns validated OAuth credentials from request params, or undefined when absent or invalid. */
export function oauthCredentials(value: unknown): OAuthCredentials | undefined {
	if (
		!isRecord(value) ||
		typeof value["access_token"] !== "string" ||
		value["access_token"].trim().length === 0 ||
		value["type"] !== "oauth"
	) {
		return undefined;
	}
	return { ...value, access_token: value["access_token"], type: "oauth" };
}

/** Returns validated API-key credentials, or undefined when absent or invalid. */
export function apiKeyCredentials(value: unknown): ApiKeyCredentials | undefined {
	if (
		!isRecord(value) ||
		typeof value["api_key"] !== "string" ||
		value["api_key"].trim().length === 0 ||
		value["type"] !== "api_key"
	) {
		return undefined;
	}
	return { ...value, api_key: value["api_key"], type: "api_key" };
}

/** Requires credentials for a protocol method that cannot run unauthenticated. */
export function requireCredentials<TCredentials extends ProviderCredentials>(
	credentials: TCredentials | undefined,
	provider: string,
): TCredentials {
	if (credentials === undefined) throw new Error(`${provider} credentials are required`);
	return credentials;
}

/**
 * Requires validated OAuth credentials for methods that cannot run unauthenticated.
 *
 * Throws an Error naming the provider, which the transport surfaces as a JSON-RPC error.
 */
export function requireOauthCredentials(
	credentials: OAuthCredentials | undefined,
	provider: string,
): OAuthCredentials {
	if (credentials === undefined) throw new Error(`${provider} OAuth credentials are required`);
	return credentials;
}
