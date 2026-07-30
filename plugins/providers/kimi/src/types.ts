/** Shared provider-domain types re-exported from the protocol SDK, plus Kimi's model shape. */
export {
	type ChatMessage,
	type ChatRequest,
	type Credentials,
	type ImageAttachment,
	imageAttachments,
	imageDataUrl,
	isRecord,
	type Json,
	type Notify,
	type ToolDefinition,
	type ToolResult,
} from "@misy/provider-sdk";

/** A model returned from the Kimi catalog. */
export type Model = {
	id: string;
	display_name: string;
	context_window: number;
	input_modalities: ("text" | "image")[];
	description?: string;
	pricing?: string;
	thinking?: {
		default: string;
		levels: { id: string; description: string }[];
	};
};
