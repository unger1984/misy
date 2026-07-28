/**
 * Protocol-domain types shared by TypeScript provider plugins.
 *
 * These types model the Misy provider protocol boundary: values arrive from the core as
 * `unknown` JSON and are narrowed once, here, before any provider adapter sees them.
 */

/** JSON values retained verbatim in Misy's opaque credential store. */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/** OAuth credentials the core echoes back opaquely to the provider that issued them. */
export type Credentials = {
	access_token: string;
	refresh_token?: string;
	expires_at?: number;
	type: "oauth";
	[key: string]: Json | undefined;
};

/** A core-owned tool definition forwarded to the remote model. */
export type ToolDefinition = {
	name: string;
	description: string;
	input_schema: Json;
};

/** A validated `chat.start` request handed to the provider adapter. */
export type ChatRequest = {
	model_id: string;
	messages: readonly Record<string, Json>[];
	tools: readonly ToolDefinition[];
	credentials: Credentials;
};

/** Delivers one stream notification correlated to its `chat.start` JSON-RPC request. */
export type Notify = (method: string, params: Record<string, Json>) => void;

/** Narrows untrusted JSON or process input to a key-value record. */
export function isRecord(value: unknown): value is Record<string, Json> {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}
