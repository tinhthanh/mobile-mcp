import path from "node:path";
import os from "node:os";
import fs from "node:fs";
import { execFileSync } from "node:child_process";

import { trace } from "./logger";

const SOURCE = path.join(__dirname, "..", "native", "ocr.swift");

const cacheDir = (): string => path.join(os.homedir(), "Library", "Caches", "mobile-mcp-lite");

let binary: string | null | undefined;

// apple vision ocr, macOS only. compiled once from native/ocr.swift and cached,
// returns null when unavailable so callers can skip ocr silently.
const ocrBinary = (): string | null => {
	if (binary !== undefined) {
		return binary;
	}

	binary = null;
	if (process.env.MOBILEMCP_OCR_BIN) {
		binary = process.env.MOBILEMCP_OCR_BIN;
		return binary;
	}

	if (process.platform !== "darwin" || process.env.MOBILEMCP_DISABLE_OCR) {
		return binary;
	}

	const target = path.join(cacheDir(), "ocr");
	try {
		const stale = !fs.existsSync(target) || fs.statSync(target).mtimeMs < fs.statSync(SOURCE).mtimeMs;
		if (stale) {
			fs.mkdirSync(cacheDir(), { recursive: true });
			execFileSync("swiftc", ["-O", SOURCE, "-o", target], { stdio: "pipe", timeout: 120000 });
		}

		binary = target;
	} catch (err: any) {
		trace(`ocr unavailable: ${err.message}`);
	}

	return binary;
};

export const ocrAvailable = (): boolean => ocrBinary() !== null;

// returns "text|x,y" lines in device pixels, or null when ocr is unavailable
export const ocrLines = (png: Buffer, statusBarHeight?: number): string[] | null => {
	const bin = ocrBinary();
	if (!bin) {
		return null;
	}

	const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mobile-mcp-ocr-"));
	const file = path.join(dir, "screen.png");
	try {
		fs.writeFileSync(file, png);
		const env = { ...process.env };
		if (statusBarHeight !== undefined) {
			env.STATUSBAR = String(statusBarHeight);
		}

		return execFileSync(bin, [file], { env, timeout: 30000 })
			.toString()
			.split("\n")
			.filter(line => line.trim() !== "")
			.map(line => " " + line);
	} finally {
		fs.rmSync(dir, { recursive: true, force: true });
	}
};
