/** Kimi client fingerprint headers and best-effort installation device identity persistence. */
import { randomUUID } from "node:crypto";
import { chmodSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { arch, hostname, platform, release, version } from "node:os";
import { join } from "node:path";
import type { ProviderConfig } from "./config";

const DEVICE_ID_FILE = "kimi-device-id";

/** Builds sanitized headers expected by Kimi OAuth and coding endpoints. */
export class KimiHeaders {
	private deviceId: string | undefined;

	/** Creates headers using the configured package version and provider-owned data directory. */
	constructor(private readonly config: ProviderConfig) {}

	/** Returns a stable install identity, or a process identity when persistence fails. */
	common(): Record<string, string> {
		return {
			"User-Agent": `KimiCLI/${sanitizeHeaderValue(this.config.packageVersion, "unknown")}`,
			"X-Msh-Platform": "kimi_cli",
			"X-Msh-Version": sanitizeHeaderValue(this.config.packageVersion, "unknown"),
			"X-Msh-Device-Name": sanitizeHeaderValue(hostname(), "unknown"),
			"X-Msh-Device-Model": sanitizeHeaderValue(deviceModel(), "unknown"),
			"X-Msh-Os-Version": sanitizeHeaderValue(version(), "unknown"),
			"X-Msh-Device-Id": sanitizeHeaderValue(this.installationDeviceId(), "unknown"),
		};
	}

	private installationDeviceId(): string {
		if (this.deviceId) return this.deviceId;
		const devicePath = join(this.config.dataDir, DEVICE_ID_FILE);
		try {
			const stored = readFileSync(devicePath, "utf8").trim();
			if (stored) return this.storeInMemory(stored);
		} catch {
			// A missing or unreadable provider data directory must not block all remote requests.
		}
		const generated = randomUUID().replaceAll("-", "");
		try {
			mkdirSync(this.config.dataDir, { recursive: true, mode: 0o700 });
			writeFileSync(devicePath, `${generated}\n`, { encoding: "utf8", mode: 0o600 });
			chmodSync(devicePath, 0o600);
		} catch {
			// The generated identity remains valid for this process when storage is unavailable.
		}
		return this.storeInMemory(generated);
	}

	private storeInMemory(deviceId: string): string {
		this.deviceId = deviceId;
		return deviceId;
	}
}

/** Removes bytes that HTTP header validation rejects and supplies a safe fallback. */
export function sanitizeHeaderValue(value: string, fallback = "unknown"): string {
	const sanitized = value.replace(/[^\x20-\x7e]/g, "").trim();
	return sanitized || fallback;
}

function deviceModel(): string {
	const system =
		platform() === "darwin" ? "macOS" : platform() === "win32" ? "Windows" : platform();
	return [system, release(), arch()].filter(Boolean).join(" ");
}
