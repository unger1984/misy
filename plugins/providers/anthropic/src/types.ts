/** Shared typed values at the boundary between the protocol and Anthropic's API. */

/** JSON values retained as opaque provider credentials and metadata. */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/** OAuth credentials accepted by Anthropic subscription endpoints. */
export type Credentials = {
	access_token: string;
	refresh_token?: string;
	expires_at?: number;
	type: "oauth";
	[key: string]: Json | undefined;
};

/** One Misy tool definition translated into the Anthropic messages format. */
export type ToolDefinition = {
	name: string;
	description: string;
	input_schema: Json;
};

/** A normalized request the provider can send to Anthropic. */
export type ChatRequest = {
	model_id: string;
	messages: readonly Record<string, unknown>[];
	tools: readonly ToolDefinition[];
	credentials: Credentials;
};

/** Delivers a stream event correlated to one JSON-RPC request. */
export type Notify = (method: string, params: Record<string, Json>) => void;

/** Narrows untrusted JSON values to key-value records. */
export function isRecord(value: unknown): value is Record<string, Json> {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}
