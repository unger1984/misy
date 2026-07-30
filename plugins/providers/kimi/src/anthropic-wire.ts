/** Kimi's Anthropic-compatible Messages request and SSE translation. */

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

type EventResult = {
	metadata: Json;
	stopped: boolean;
};

/** Builds a Kimi request for models whose catalog protocol is Anthropic-compatible. */
export function createAnthropicRequest(
	model: string,
	messages: readonly Record<string, unknown>[],
	tools: readonly ToolDefinition[],
	maxOutputTokens?: number,
): Record<string, Json> {
	const system = messages
		.filter((message) => message["role"] === "system")
		.map(messageText)
		.filter((value) => value.length > 0)
		.join("\n\n");
	return {
		model,
		max_tokens: maxOutputTokens ?? 32_000,
		stream: true,
		...(system.length > 0 ? { system } : {}),
		messages: messages.filter((message) => message["role"] !== "system").map(messageBody),
		tools: tools.map((tool) => ({
			name: tool.name,
			description: tool.description,
			input_schema: tool.input_schema,
		})),
	};
}

/** Converts Kimi's Anthropic-compatible SSE events to correlated Misy notifications. */
export async function notifyAnthropicEvents(
	stream: ReadableStream<Uint8Array>,
	requestId: number,
	notify: Notify,
): Promise<Json> {
	const reader = stream.getReader();
	const decoder = new TextDecoder();
	const tools = new Map<number, ToolState>();
	let pending = "";
	let metadata: Json = { completed: false };
	let stopped = false;
	try {
		while (true) {
			const read = await reader.read();
			pending += decoder.decode(read.value, { stream: !read.done });
			const frames = pending.split(/\r?\n\r?\n/);
			pending = frames.pop() ?? "";
			for (const frame of frames) {
				const processed = processFrame(frame, tools, requestId, notify, metadata);
				metadata = processed.metadata;
				stopped ||= processed.stopped;
			}
			if (read.done) break;
		}
		if (pending.trim().length > 0) {
			throw new Error("Kimi Anthropic stream ended mid-event");
		}
		if (!stopped) throw new Error("Kimi Anthropic stream ended before message_stop");
	} finally {
		reader.releaseLock();
	}
	return metadata;
}

function messageBody(message: Record<string, unknown>): Json {
	if (message["role"] === "tool") return toolResultMessage(message);
	const blocks = messageContent(message);
	if (message["role"] === "assistant") blocks.push(...assistantToolBlocks(message));
	return {
		role: message["role"] === "assistant" ? "assistant" : "user",
		content: blocks.length > 0 ? blocks : messageText(message),
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
				tool_use_id: stringValue(entry["tool_call_id"]),
				content: richContent(stringValue(entry["content"]), imageAttachments(entry["attachments"])),
				is_error: entry["is_error"] === true,
			};
		}),
	};
}

function messageContent(message: Record<string, unknown>): Json[] {
	return richContent(messageText(message), imageAttachments(message["attachments"]));
}

function richContent(text: string, attachments: readonly ImageAttachment[]): Json[] {
	const blocks: Json[] = text.length > 0 ? [{ type: "text", text }] : [];
	for (const attachment of attachments) {
		blocks.push({
			type: "image",
			source: {
				type: "base64",
				media_type: attachment.media_type,
				data: attachment.data_base64,
			},
		});
	}
	return blocks;
}

function assistantToolBlocks(message: Record<string, unknown>): Json[] {
	const calls = Array.isArray(message["tool_calls"]) ? message["tool_calls"] : [];
	return calls.map((call) => {
		const entry = record(call);
		return {
			type: "tool_use",
			id: stringValue(entry["id"]),
			name: stringValue(entry["name"]),
			input: json(entry["arguments"]),
		};
	});
}

function processFrame(
	frame: string,
	tools: Map<number, ToolState>,
	requestId: number,
	notify: Notify,
	metadata: Json,
): EventResult {
	const event = frame.match(/^event:\s*(.+)$/m)?.[1] ?? "message";
	const raw = frame.match(/^data:\s*(.+)$/m)?.[1] ?? "{}";
	const data = parseEvent(raw);
	if (event === "content_block_start") startTool(data, tools);
	if (event === "content_block_delta") sendDelta(data, tools, requestId, notify);
	if (event === "content_block_stop") sendTool(data, tools, requestId, notify);
	if (event === "message_stop") {
		notify("completed", { request_id: requestId, metadata: data });
		return { metadata: data, stopped: true };
	}
	if (event === "error") {
		const error = record(data["error"]);
		throw new Error(
			typeof error["message"] === "string" ? error["message"] : "Kimi Anthropic stream failed",
		);
	}
	return { metadata, stopped: false };
}

function startTool(data: Record<string, Json>, tools: Map<number, ToolState>): void {
	const index = data["index"];
	const block = record(data["content_block"]);
	if (typeof index !== "number" || block["type"] !== "tool_use") return;
	if (typeof block["id"] !== "string" || typeof block["name"] !== "string") return;
	const input = block["input"];
	tools.set(index, {
		id: block["id"],
		name: block["name"],
		input: isRecord(input) && Object.keys(input).length === 0 ? "" : JSON.stringify(input ?? {}),
	});
}

function sendDelta(
	data: Record<string, Json>,
	tools: Map<number, ToolState>,
	requestId: number,
	notify: Notify,
): void {
	const delta = record(data["delta"]);
	if (typeof delta["text"] === "string") {
		notify("text_delta", { request_id: requestId, delta: delta["text"] });
		return;
	}
	const state = typeof data["index"] === "number" ? tools.get(data["index"]) : undefined;
	if (state !== undefined && typeof delta["partial_json"] === "string") {
		state.input += delta["partial_json"];
	}
}

function sendTool(
	data: Record<string, Json>,
	tools: Map<number, ToolState>,
	requestId: number,
	notify: Notify,
): void {
	const index = data["index"];
	const state = typeof index === "number" ? tools.get(index) : undefined;
	if (state === undefined || typeof index !== "number") return;
	tools.delete(index);
	notify("tool_call", {
		request_id: requestId,
		id: state.id,
		name: state.name,
		arguments: parseArguments(state.input),
	});
}

function parseEvent(raw: string): Record<string, Json> {
	try {
		const value: unknown = JSON.parse(raw);
		return record(value);
	} catch {
		throw new Error("Kimi Anthropic stream contained invalid JSON");
	}
}

function parseArguments(raw: string): Json {
	if (raw.length === 0) return {};
	try {
		const value: unknown = JSON.parse(raw);
		return json(value);
	} catch {
		return { raw };
	}
}

function record(value: unknown): Record<string, Json> {
	return isRecord(value) ? value : {};
}

function messageText(message: Record<string, unknown>): string {
	return typeof message["content"] === "string" ? message["content"] : "";
}

function stringValue(value: Json | undefined): string {
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
