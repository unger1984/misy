/** OpenAI provider process entry point: wires the provider adapter to the protocol SDK. */
import { preferredDefaultModel, requireOauthCredentials, serve } from "@misy/provider-sdk";
import { DEFAULT_MODEL_ID } from "./model-catalog";
import { OpenAiProvider } from "./provider";

const provider = new OpenAiProvider();

serve({
	name: "OpenAI",
	requestFailureMessage: "OpenAI provider request failed",
	authStatus: (credentials) => provider.authStatus(credentials),
	startAuth: async (method) => {
		const started = await provider.startAuth(method);
		return { kind: "browser", url: started.url, session: started.session };
	},
	completeAuth: (session, completion) => provider.completeAuth(session, completion),
	refreshAuth: (credentials) => provider.refreshAuth(credentials),
	logout: () => provider.logout(),
	listModels: async (credentials) => {
		const models = await provider.listModels(requireOauthCredentials(credentials, "OpenAI"));
		const defaultModel = preferredDefaultModel(models, DEFAULT_MODEL_ID);
		return defaultModel === undefined ? { models } : { models, default_model: defaultModel };
	},
	usage: (credentials) => provider.usage(credentials),
	chat: (request, requestId, notify, signal) =>
		provider.streamChat(request, requestId, notify, signal),
});
