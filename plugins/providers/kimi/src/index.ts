/** Kimi provider process entry point: wires the provider adapter to the protocol SDK. */
import { oauthCredentials, serve } from "@misy/provider-sdk";
import { KimiProvider } from "./provider";

const provider = new KimiProvider();

serve({
	name: "Kimi",
	requestFailureMessage: "Kimi provider request failed",
	parseCredentials: oauthCredentials,
	authStatus: (credentials) => provider.authStatus(credentials),
	startAuth: (method) => provider.startAuth(method),
	// Device flows intentionally ignore completion data; the SDK still validates its shape.
	completeAuth: (session) => provider.completeAuth(session),
	refreshAuth: (credentials) => provider.refreshAuth(credentials),
	logout: () => provider.logout(),
	listModels: async (credentials) => {
		const models = await provider.listModels(credentials);
		return {
			models,
			default_model: provider.defaultModel(models),
			source: provider.catalogSource(),
		};
	},
	usage: (credentials) => provider.usage(credentials),
	chat: (request, requestId, notify, signal) =>
		provider.streamChat(request, requestId, notify, signal),
	shutdown: () => provider.cancelAuthentication(),
});
