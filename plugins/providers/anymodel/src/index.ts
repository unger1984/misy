/** AnyModel provider process entry point. */
import { apiKeyCredentials, serve } from "@misy/provider-sdk";
import { AnyModelProvider } from "./provider";

const provider = new AnyModelProvider();

serve({
	name: "AnyModel",
	requestFailureMessage: "AnyModel provider request failed",
	parseCredentials: apiKeyCredentials,
	authStatus: (credentials) => provider.authStatus(credentials),
	startAuth: async (method) => provider.startAuth(method),
	completeAuth: (session, completion) => provider.completeAuth(session, completion),
	refreshAuth: (credentials) => provider.refreshAuth(credentials),
	logout: () => provider.logout(),
	listModels: async (credentials) => {
		const models = await provider.listModels(credentials);
		return { models, default_model: provider.defaultModel(models) };
	},
	usage: () => provider.usage(),
	chat: (request, requestId, notify, signal) =>
		provider.streamChat(request, requestId, notify, signal),
});
