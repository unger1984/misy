/**
 * Minimal provider adapter used by the SDK transport tests.
 *
 * It speaks no real provider API: every method echoes just enough to observe how the transport
 * validated and forwarded the request. `chat` blocks until aborted unless the model id is "fast",
 * so tests can exercise `chat.cancel`. The shutdown hook touches the file named by
 * `FIXTURE_SHUTDOWN_FILE` so tests can observe the EOF lifecycle.
 */
import { writeFileSync } from "node:fs";
import { serve } from "../src/index";

serve({
	name: "Fixture",
	requestFailureMessage: "Fixture request failed",
	authStatus: (credentials) => ({ authenticated: credentials !== undefined }),
	startAuth: async (method) => await Promise.resolve({ kind: "none", method }),
	completeAuth: async (session, completion) =>
		await Promise.resolve({
			credentials: { access_token: "fixture-token", type: "oauth" },
			session: session ?? null,
			completion,
		}),
	refreshAuth: async (credentials) => await Promise.resolve({ credentials }),
	logout: () => ({}),
	listModels: async (credentials) =>
		await Promise.resolve({
			models: [{ id: "fixture-model" }],
			default_model: "fixture-model",
			authenticated: credentials !== undefined,
		}),
	usage: async (credentials) =>
		await Promise.resolve({ fetched_at: 1, limits: [], echoed: credentials }),
	chat: async (request, requestId, notify, signal) => {
		notify("text_delta", { request_id: requestId, delta: request.model_id });
		if (request.model_id !== "fast") {
			await new Promise<void>((resolve) => {
				if (signal.aborted) resolve();
				else signal.addEventListener("abort", () => resolve(), { once: true });
			});
		}
		return {
			metadata: { completed: request.model_id === "fast", cancelled: signal.aborted },
			credentials: request.credentials,
		};
	},
	shutdown: () => {
		const marker = process.env["FIXTURE_SHUTDOWN_FILE"];
		if (marker !== undefined && marker.length > 0) writeFileSync(marker, "shutdown\n");
	},
});
