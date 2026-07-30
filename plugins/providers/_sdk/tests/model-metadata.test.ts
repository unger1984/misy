import { expect, test } from "bun:test";
import { referencePricing } from "../src/model-metadata";

test("resolves generic vendor reference prices", () => {
	expect(referencePricing("gpt-5.6-sol-review")).toBe("$5/30");
	expect(referencePricing("gemini-3-5-flash-agent")).toBe("$1.5/9");
	expect(referencePricing("free")).toBe("free");
	expect(referencePricing("unknown/model")).toBeUndefined();
});
