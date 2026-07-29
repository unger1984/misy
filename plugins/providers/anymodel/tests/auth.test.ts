import { expect, test } from "bun:test";
import { authStatus, completeAuth, startAuth } from "../src/auth";

test("starts only the masked API-key prompt and validates its exact session marker", () => {
	expect(startAuth("api_key")).toEqual({
		kind: "prompt",
		fields: [{ id: "api_key", label: "API key", secret: true }],
		session: { method: "api_key" },
	});
	expect(() => startAuth("oauth")).toThrow("Unsupported AnyModel authentication method: oauth");
	expect(() => completeAuth({ method: "other" }, { api_key: "secret" })).toThrow(
		"Invalid AnyModel authentication session",
	);
});

test("trims a submitted API key, rejects blanks, and does not expose it through status", () => {
	const credentials = completeAuth({ method: "api_key" }, { api_key: "  key-with-punctuation!  " });
	expect(credentials).toEqual({ type: "api_key", api_key: "key-with-punctuation!" });
	expect(authStatus(credentials)).toEqual({ authenticated: true });
	expect(authStatus(undefined)).toEqual({ authenticated: false });
	expect(() => completeAuth({ method: "api_key" }, { api_key: " \t " })).toThrow(
		"AnyModel API key is required",
	);
});
