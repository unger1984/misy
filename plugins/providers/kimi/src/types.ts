/** Shared provider-domain types, independent from the JSON-RPC transport. */

/** JSON values accepted by Misy's opaque credential store. */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/** Kimi OAuth credentials used by its coding API. */
export type Credentials = {
	access_token: string;
	refresh_token?: string;
	expires_at?: number;
	type: "oauth";
	[key: string]: Json | undefined;
};

/** A normalized Misy chat request. */
export type ChatRequest = {
	model_id: string;
	messages: readonly Record<string, unknown>[];
	tools: readonly ToolDefinition[];
	credentials: Credentials;
};

/** A core-owned function exposed to the remote model. */
export type ToolDefinition = {
	name: string;
	description: string;
	input_schema: Json;
};

/** A model returned from the Kimi catalog. */
export type Model = {
	id: string;
	display_name: string;
	context_window: number;
};

/** Sends a stream notification to the Misy host. */
export type Notify = (method: string, params: Record<string, unknown>) => void;

/** Narrows untrusted JSON or process input to a record. */
export function isRecord(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}
