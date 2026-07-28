/**
 * Validation of the opaque OAuth credentials the core echoes back inside request params.
 *
 * Credentials cross a process boundary, so they are `unknown` JSON until narrowed here. A cast is
 * not validation: only the two fields the protocol relies on are checked, and every other field is
 * preserved untouched for the issuing provider.
 */
import { type Credentials, isRecord, type Json } from "./types";

/** Returns validated OAuth credentials from request params, or undefined when absent or invalid. */
export function oauthCredentials(params: Record<string, Json>): Credentials | undefined {
	const value = params["credentials"];
	if (!isRecord(value) || typeof value["access_token"] !== "string" || value["type"] !== "oauth") {
		return undefined;
	}
	return { ...value, access_token: value["access_token"], type: "oauth" };
}

/**
 * Requires validated OAuth credentials for methods that cannot run unauthenticated.
 *
 * Throws an Error naming the provider, which the transport surfaces as a JSON-RPC error.
 */
export function requireOauthCredentials(
	credentials: Credentials | undefined,
	provider: string,
): Credentials {
	if (credentials === undefined) throw new Error(`${provider} OAuth credentials are required`);
	return credentials;
}
