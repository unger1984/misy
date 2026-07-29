/** Anthropic provider process entry point: wires the provider adapter to the protocol SDK. */
import { oauthCredentials, requireOauthCredentials, serve } from "@misy/provider-sdk";
import { AnthropicProvider } from "./provider";

const provider = new AnthropicProvider();

serve({
	name: "Anthropic",
	requestFailureMessage: "Anthropic request failed",
	parseCredentials: oauthCredentials,
	authStatus: (credentials) => provider.authStatus(credentials),
	startAuth: async (method) => {
		const started = await provider.startAuth(method);
		return { kind: "browser", url: started.url, session: started.session };
	},
	completeAuth: (session, completion) => provider.completeAuth(session, completion),
	refreshAuth: (credentials) => provider.refreshAuth(credentials),
	logout: () => provider.logout(),
	listModels: (credentials) =>
		provider.listModels(requireOauthCredentials(credentials, "Anthropic")),
	usage: (credentials) => provider.usage(credentials),
	chat: (request, requestId, notify, signal) =>
		provider.streamChat(request, requestId, notify, signal),
});
