/** Anthropic Messages request translation and SSE event normalization. */
import {
	type ImageAttachment,
	imageAttachments,
	isRecord,
	type Json,
	type Notify,
	type ToolDefinition,
} from "./types";

type ToolState = {
	id: string;
	name: string;
	input: string;
};

/** Builds the streaming Anthropic Messages request body from normalized Misy messages. */
export function createMessagesRequest(
	model: string,
	messages: readonly Record<string, unknown>[],
	tools: readonly ToolDefinition[],
): Record<string, Json> {
	const system = messages
		.filter((message) => message["role"] === "system")
		.map((message) => content(message))
		.filter((value) => value.length > 0)
		.join("\n\n");
	return {
		model,
		max_tokens: 64_000,
		stream: true,
		...(system.length > 0 ? { system } : {}),
		messages: messages.filter((message) => message["role"] !== "system").map(messageBody),
		tools: tools.map(toolBody),
	};
}

/** Converts Anthropic SSE events into Misy notifications and completion metadata. */
export async function notifyMessageEvents(
	stream: ReadableStream<Uint8Array>,
	requestId: number,
	notify: Notify,
): Promise<Json> {
	const reader = stream.getReader();
	const decoder = new TextDecoder();
	const toolBlocks = new Map<number, ToolState>();
	let pending = "";
	let metadata: Json = { completed: false };
	try {
		while (true) {
			const read = await reader.read();
			pending += decoder.decode(read.value, { stream: !read.done });
			const parts = pending.split(/\r?\n\r?\n/);
			pending = parts.pop() ?? "";
			for (const part of parts) {
				metadata = handleEvent(part, requestId, notify, toolBlocks, metadata);
			}
			if (read.done) break;
		}
		// SSE events end with a blank line, so a non-empty remainder at EOF means the
		// connection dropped mid-event; completing silently would hide the truncation.
		if (pending.trim().length > 0) {
			throw new Error("Anthropic Messages stream ended mid-event");
		}
	} finally {
		reader.releaseLock();
	}
	return metadata;
}

function messageBody(message: Record<string, unknown>): Json {
	if (message["role"] === "tool") return toolResultMessage(message);
	const blocks = toolUseBlocks(message);
	if (message["role"] === "user") blocks.unshift(...messageContentBlocks(message));
	return {
		role: message["role"] === "assistant" ? "assistant" : "user",
		content: blocks.length > 0 ? blocks : content(message),
	};
}

function toolResultMessage(message: Record<string, unknown>): Json {
	const results = Array.isArray(message["tool_results"]) ? message["tool_results"] : [];
	return {
		role: "user",
		content: results.map((result) => {
			const entry = record(result);
			return {
				type: "tool_result",
				tool_use_id: text(entry["tool_call_id"]),
				content: toolResultContent(entry),
				is_error: entry["is_error"] === true,
			};
		}),
	};
}

function messageContentBlocks(message: Record<string, unknown>): Json[] {
	const attachments = imageAttachments(message["attachments"]);
	return attachments.length === 0 ? [] : contentBlocks(content(message), attachments);
}

function toolResultContent(result: Record<string, Json>): Json {
	const attachments = imageAttachments(result["attachments"]);
	if (attachments.length === 0) return text(result["content"]);
	return contentBlocks(text(result["content"]), attachments);
}

function contentBlocks(text: string, attachments: readonly ImageAttachment[]): Json[] {
	const blocks: Json[] = text.length > 0 ? [{ type: "text", text }] : [];
	for (const attachment of attachments) blocks.push(imageBlock(attachment));
	return blocks;
}

function imageBlock(attachment: ImageAttachment): Json {
	return {
		type: "image",
		source: {
			type: "base64",
			media_type: attachment.media_type,
			data: attachment.data_base64,
		},
	};
}

function toolUseBlocks(message: Record<string, unknown>): Json[] {
	const calls = Array.isArray(message["tool_calls"]) ? message["tool_calls"] : [];
	return calls.map((call) => {
		const entry = record(call);
		return {
			type: "tool_use",
			id: text(entry["id"]),
			name: text(entry["name"]),
			input: json(entry["arguments"]),
		};
	});
}

function toolBody(tool: ToolDefinition): Json {
	return { name: tool.name, description: tool.description, input_schema: tool.input_schema };
}

function handleEvent(
	part: string,
	requestId: number,
	notify: Notify,
	toolBlocks: Map<number, ToolState>,
	metadata: Json,
): Json {
	const event = part.match(/^event:\s*(.+)$/m)?.[1] ?? "message";
	const raw = part.match(/^data:\s*(.+)$/m)?.[1] ?? "{}";
	const data = parseEvent(raw);
	if (event === "content_block_start") startTool(data, toolBlocks);
	if (event === "content_block_delta") sendDelta(data, requestId, notify, toolBlocks);
	if (event === "content_block_stop") sendTool(data, requestId, notify, toolBlocks);
	if (event === "message_stop") {
		notify("completed", { request_id: requestId, metadata: data });
		return data;
	}
	if (event === "error") throw new Error(eventMessage(data));
	return metadata;
}

function startTool(data: Record<string, Json>, toolBlocks: Map<number, ToolState>): void {
	const index = data["index"];
	const contentBlock = record(data["content_block"]);
	if (typeof index !== "number" || contentBlock["type"] !== "tool_use") return;
	const id = contentBlock["id"];
	const name = contentBlock["name"];
	if (typeof id !== "string" || typeof name !== "string") return;
	const input = contentBlock["input"];
	toolBlocks.set(index, {
		id,
		name,
		input: isRecord(input) && Object.keys(input).length === 0 ? "" : JSON.stringify(input ?? {}),
	});
}

function sendDelta(
	data: Record<string, Json>,
	requestId: number,
	notify: Notify,
	toolBlocks: Map<number, ToolState>,
): void {
	const delta = record(data["delta"]);
	if (typeof delta["text"] === "string") {
		notify("text_delta", { request_id: requestId, delta: delta["text"] });
		return;
	}
	const index = data["index"];
	const state = typeof index === "number" ? toolBlocks.get(index) : undefined;
	if (state !== undefined && typeof delta["partial_json"] === "string")
		state.input += delta["partial_json"];
}

function sendTool(
	data: Record<string, Json>,
	requestId: number,
	notify: Notify,
	toolBlocks: Map<number, ToolState>,
): void {
	const index = data["index"];
	const state = typeof index === "number" ? toolBlocks.get(index) : undefined;
	if (state === undefined || typeof index !== "number") return;
	toolBlocks.delete(index);
	notify("tool_call", {
		request_id: requestId,
		id: state.id,
		name: state.name,
		arguments: parseArguments(state.input),
	});
}

function parseEvent(raw: string): Record<string, Json> {
	try {
		const parsed: unknown = JSON.parse(raw);
		return isRecord(parsed) ? parsed : {};
	} catch {
		throw new Error("Anthropic Messages stream contained invalid JSON");
	}
}

function parseArguments(raw: string): Json {
	try {
		const parsed: unknown = JSON.parse(raw);
		return json(parsed);
	} catch {
		return { raw };
	}
}

function eventMessage(data: Record<string, Json>): string {
	const error = record(data["error"]);
	return typeof error["message"] === "string"
		? error["message"]
		: "Anthropic Messages stream failed";
}

function content(message: Record<string, unknown>): string {
	return typeof message["content"] === "string" ? message["content"] : "";
}

function record(value: unknown): Record<string, Json> {
	return isRecord(value) ? value : {};
}

function text(value: Json | undefined): string {
	return typeof value === "string" ? value : "";
}

function json(value: unknown): Json {
	if (
		value === null ||
		typeof value === "boolean" ||
		typeof value === "number" ||
		typeof value === "string"
	) {
		return value;
	}
	if (Array.isArray(value)) return value.map(json);
	if (value !== null && typeof value === "object") {
		return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, json(entry)]));
	}
	return null;
}
