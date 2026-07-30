/** Shared SDK types plus AnyModel's normalized model shape. */
export {
	type ApiKeyCredentials,
	type ChatRequest,
	type ImageAttachment,
	imageAttachments,
	imageDataUrl,
	isRecord,
	type Json,
	type Notify,
	type ToolDefinition,
} from "@misy/provider-sdk";

/** A model normalized from AnyModel's authenticated upstream catalog. */
export type Model = {
	id: string;
	display_name: string;
	context_window: number;
	input_modalities: ("text" | "image")[];
	description?: string;
	pricing?: string;
};
