/** Shared provider-domain types re-exported from the protocol SDK, plus Kimi's model shape. */
export {
	type ChatRequest,
	type Credentials,
	isRecord,
	type Json,
	type Notify,
	type ToolDefinition,
} from "@misy/provider-sdk";

/** A model returned from the Kimi catalog. */
export type Model = {
	id: string;
	display_name: string;
	context_window: number;
};
