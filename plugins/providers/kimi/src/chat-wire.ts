/** OpenAI-compatible Kimi chat request construction and SSE stream normalization. */
import {
	type ImageAttachment,
	imageAttachments,
	imageDataUrl,
	isRecord,
	type Json,
	type Notify,
	type ToolDefinition,
} from "./types";

type ToolAccumulator = {
	id: string;
	name: string;
	arguments: string;
};

type ToolImageGroup = {
	toolCallId: string;
	attachments: ImageAttachment[];
};

/** Builds the OpenAI-compatible streaming request accepted by Kimi's coding API. */
export function createChatRequest(
	model: string,
	messages: readonly Record<string, unknown>[],
	tools: readonly ToolDefinition[],
	maxOutputTokens?: number,
): Record<string, unknown> {
	return {
		model,
		messages: requestMessages(messages),
		tools: tools.map((tool) => ({
			type: "function",
			function: {
				name: tool.name,
				description: tool.description,
				parameters: tool.input_schema,
			},
		})),
		stream: true,
		...(maxOutputTokens === undefined ? {} : { max_tokens: maxOutputTokens }),
	};
}

function requestMessages(
	messages: readonly Record<string, unknown>[],
): readonly Record<string, unknown>[] {
	if (!messages.some(needsMapping)) return messages;
	const mapped: Record<string, unknown>[] = [];
	for (const message of messages) {
		const attachments = imageAttachments(message["attachments"]);
		if (message["role"] === "user" && attachments.length > 0) {
			mapped.push(userImageMessage(message, attachments));
			continue;
		}
		const toolImages = toolResultImageGroups(message);
		const withoutMessageAttachments = stripMessageAttachments(message);
		mapped.push(
			toolResultsHaveAttachments(message)
				? stripToolResultAttachments(withoutMessageAttachments)
				: withoutMessageAttachments,
		);
		for (const group of toolImages) mapped.push(toolImageUserMessage(group));
	}
	return mapped;
}

function needsMapping(message: Record<string, unknown>): boolean {
	return Object.hasOwn(message, "attachments") || toolResultsHaveAttachments(message);
}

function userImageMessage(
	message: Record<string, unknown>,
	attachments: readonly ImageAttachment[],
): Record<string, unknown> {
	const mapped = { ...message };
	delete mapped["attachments"];
	const text = typeof message["content"] === "string" ? message["content"] : "";
	mapped["content"] = openAiContent(text, attachments);
	return mapped;
}

function toolImageUserMessage(group: ToolImageGroup): Record<string, unknown> {
	const label = group.toolCallId
		? `Images from tool result ${group.toolCallId}:`
		: "Images from a tool result:";
	return { role: "user", content: openAiContent(label, group.attachments) };
}

function stripMessageAttachments(message: Record<string, unknown>): Record<string, unknown> {
	if (!Object.hasOwn(message, "attachments")) return message;
	const mapped = { ...message };
	delete mapped["attachments"];
	return mapped;
}

function openAiContent(text: string, attachments: readonly ImageAttachment[]): Json[] {
	const content: Json[] = text.length > 0 ? [{ type: "text", text }] : [];
	for (const attachment of attachments) {
		content.push({ type: "image_url", image_url: { url: imageDataUrl(attachment) } });
	}
	return content;
}

function toolResultImageGroups(message: Record<string, unknown>): ToolImageGroup[] {
	if (message["role"] !== "tool" || !Array.isArray(message["tool_results"])) return [];
	return message["tool_results"].flatMap((result) => toolImageGroup(result));
}

function toolImageGroup(value: unknown): ToolImageGroup[] {
	if (!isRecord(value)) return [];
	const attachments = imageAttachments(value["attachments"]);
	if (attachments.length === 0) return [];
	return [
		{
			toolCallId: typeof value["tool_call_id"] === "string" ? value["tool_call_id"] : "",
			attachments,
		},
	];
}

function toolResultsHaveAttachments(message: Record<string, unknown>): boolean {
	if (!Array.isArray(message["tool_results"])) return false;
	return message["tool_results"].some(
		(result) => isRecord(result) && Object.hasOwn(result, "attachments"),
	);
}

function stripToolResultAttachments(message: Record<string, unknown>): Record<string, unknown> {
	const results = Array.isArray(message["tool_results"]) ? message["tool_results"] : [];
	return {
		...message,
		tool_results: results.map((result) => {
			if (!isRecord(result)) return result;
			const mapped = { ...result };
			delete mapped["attachments"];
			return mapped;
		}),
	};
}

/** Translates Kimi SSE chunks into correlated Misy text, tool, and completion notifications. */
export async function notifyChatEvents(
	stream: ReadableStream<Uint8Array>,
	requestId: number,
	notify: Notify,
): Promise<Json> {
	const reader = stream.getReader();
	const decoder = new TextDecoder();
	const tools = new Map<number, ToolAccumulator>();
	let pending = "";
	let completed = false;
	try {
		while (true) {
			const read = await reader.read();
			pending += decoder.decode(read.value, { stream: !read.done });
			const frames = pending.split(/\r?\n\r?\n/);
			pending = frames.pop() ?? "";
			for (const frame of frames) {
				if (processFrame(frame, tools, requestId, notify)) completed = true;
			}
			if (read.done) break;
		}
		if (pending.trim() && processFrame(pending, tools, requestId, notify)) completed = true;
		emitTools(tools, requestId, notify);
		const metadata: Json = { completed: true, finished_by: completed ? "done" : "eof" };
		notify("completed", { request_id: requestId, metadata });
		return metadata;
	} finally {
		reader.releaseLock();
	}
}

function processFrame(
	frame: string,
	tools: Map<number, ToolAccumulator>,
	requestId: number,
	notify: Notify,
): boolean {
	const payload = frame
		.split(/\r?\n/)
		.filter((line) => line.startsWith("data:"))
		.map((line) => line.slice("data:".length).trim())
		.join("\n");
	if (!payload) return false;
	if (payload === "[DONE]") return true;
	let value: unknown;
	try {
		value = JSON.parse(payload);
	} catch {
		throw new Error("Kimi chat stream contained invalid JSON");
	}
	if (!isRecord(value)) throw new Error("Kimi chat stream event was not an object");
	const error = isRecord(value["error"]) ? value["error"] : undefined;
	if (error) throw new Error(errorMessage(error, "Kimi chat stream failed"));
	const choices = value["choices"];
	if (!Array.isArray(choices)) return false;
	for (const choice of choices) consumeChoice(choice, tools, requestId, notify);
	return choices.some((choice) => isRecord(choice) && choice["finish_reason"] !== null);
}

function consumeChoice(
	choice: unknown,
	tools: Map<number, ToolAccumulator>,
	requestId: number,
	notify: Notify,
): void {
	if (!isRecord(choice) || !isRecord(choice["delta"])) return;
	const delta = choice["delta"];
	if (typeof delta["content"] === "string" && delta["content"]) {
		notify("text_delta", { request_id: requestId, delta: delta["content"] });
	}
	if (!Array.isArray(delta["tool_calls"])) return;
	for (const value of delta["tool_calls"]) appendToolFragment(value, tools);
}

function appendToolFragment(value: unknown, tools: Map<number, ToolAccumulator>): void {
	if (!isRecord(value) || typeof value["index"] !== "number") return;
	const index = value["index"];
	const existing = tools.get(index) ?? { id: "", name: "", arguments: "" };
	if (typeof value["id"] === "string") existing.id = value["id"];
	if (isRecord(value["function"])) {
		const functionValue = value["function"];
		if (typeof functionValue["name"] === "string") existing.name = functionValue["name"];
		if (typeof functionValue["arguments"] === "string")
			existing.arguments += functionValue["arguments"];
	}
	tools.set(index, existing);
}

function emitTools(tools: Map<number, ToolAccumulator>, requestId: number, notify: Notify): void {
	for (const tool of [...tools.entries()]
		.sort(([left], [right]) => left - right)
		.map(([, tool]) => tool)) {
		if (!tool.id || !tool.name)
			throw new Error("Kimi chat stream returned an incomplete tool call");
		notify("tool_call", {
			request_id: requestId,
			id: tool.id,
			name: tool.name,
			arguments: parseToolArguments(tool.arguments),
		});
	}
}

function parseToolArguments(raw: string): Json {
	if (!raw.trim()) return {};
	try {
		// JSON.parse can only produce Json values, so the return type states a fact;
		// the wrapper exists to shed parse's `any` without an `as` cast on boundary data.
		return JSON.parse(raw);
	} catch {
		return { raw };
	}
}

function errorMessage(error: Record<string, unknown>, fallback: string): string {
	return typeof error["message"] === "string" ? error["message"] : fallback;
}
