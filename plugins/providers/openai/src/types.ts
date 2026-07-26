/** Shared provider-domain types kept independent from JSON-RPC transport. */

/** JSON values retained verbatim in Misy's opaque credential store. */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/** OAuth credentials required by the ChatGPT-backed Responses endpoint. */
export type Credentials = {
	access_token: string;
	refresh_token?: string;
	expires_at?: number;
	chatgpt_account_id?: string;
	id_token?: string;
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

/** A local tool advertised to the Responses API. */
export type ToolDefinition = {
	name: string;
	description: string;
	input_schema: Json;
};

/** Sends one stream notification to the Misy host. */
export type Notify = (method: string, params: Record<string, unknown>) => void;
