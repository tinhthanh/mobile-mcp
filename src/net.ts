import path from "node:path";
import os from "node:os";
import fs from "node:fs";
import net from "node:net";
import { execFileSync, spawn, ChildProcess } from "node:child_process";

import { trace } from "./logger";
import { AndroidRobot } from "./android";
import { ActionableError } from "./robot";

// captures the device's http(s) traffic with mitmdump, reached over usb through
// `adb reverse`, so nothing listens on the lan and no root is needed. only apps
// that honor the system proxy show up; tls is passed through (host only) unless
// the host matches `decrypt` and the app trusts the mitmproxy ca.
const ADDON = path.join(__dirname, "..", "native", "net_addon.py");
const PCAP_HOSTS = path.join(__dirname, "..", "native", "pcap_hosts.py");

// per-app capture for apps that ignore the system proxy (zalo, flutter, custom sockets):
// pcapdroid's vpn sees every connection of the app but cannot decrypt, so it lists hosts.
// without PCAPDROID_API_KEY the device asks for consent on start and stop.
const PCAPDROID = "com.emanuelef.remote_capture";
const PCAPDROID_CTRL = `${PCAPDROID}/.activities.CaptureCtrl`;
const PCAP_DIR = "/sdcard/Download/PCAPdroid";
const LIST_LIMIT = 50;

// browser/transport noise that rarely matters when debugging an api; show {all} keeps them
const NOISY_HEADER = /^(sec-ch-|sec-fetch-|accept|priority$|upgrade-insecure-requests$|x-content-type-options$|access-control-|date$|connection$|proxy-connection$|keep-alive$|via$|alt-svc$|server-timing$|nel$|report-to$)/i;
const MAX_PATH = 80;

interface Summary {
	id: number;
	t: number;
	tls?: boolean;
	m?: string;
	s?: number;
	h: string;
	p?: string;
	q?: number;
	r?: number;
	ms?: number;
	ct?: string;
	err?: string;
}

const findMitmdump = (): string => {
	const candidates = [process.env.MOBILEMCP_MITMDUMP, "/opt/homebrew/bin/mitmdump", "/usr/local/bin/mitmdump"];
	for (const candidate of candidates) {
		if (candidate && fs.existsSync(candidate)) {
			return candidate;
		}
	}

	try {
		return execFileSync("which", ["mitmdump"]).toString().trim();
	} catch (error: any) {
		throw new ActionableError("mitmdump not found. Install mitmproxy (brew install mitmproxy) or set MOBILEMCP_MITMDUMP");
	}
};

const freePort = (): Promise<number> => new Promise((resolve, reject) => {
	const server = net.createServer();
	server.once("error", reject);
	server.listen(0, "127.0.0.1", () => {
		const port = (server.address() as net.AddressInfo).port;
		server.close(() => resolve(port));
	});
});

const isListening = (port: number): Promise<boolean> => new Promise(resolve => {
	const socket = net.connect(port, "127.0.0.1");
	socket.once("connect", () => {
		socket.destroy();
		resolve(true);
	});
	socket.once("error", () => resolve(false));
});

const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

const size = (bytes?: number): string => {
	if (!bytes) {
		return "0";
	}

	return bytes < 1024 ? `${bytes}B` : `${(bytes / 1024).toFixed(1)}KB`;
};

const readJsonl = <T>(file: string): T[] => {
	if (!fs.existsSync(file)) {
		return [];
	}

	return fs.readFileSync(file, "utf8").split("\n").filter(line => line.trim() !== "").map(line => JSON.parse(line));
};

export class NetCapture {

	private proc: ChildProcess | null = null;
	private port = 0;
	private dir = "";
	private cursor = 0;
	// set while pcapdroid captures a single app
	private pcapName = "";

	public constructor(private robot: AndroidRobot) {
	}

	public get running(): boolean {
		return this.proc !== null || this.pcapName !== "";
	}

	private get logFile() {
		return path.join(this.dir, "log.jsonl");
	}

	private get detailFile() {
		return path.join(this.dir, "detail.jsonl");
	}

	private pcapdroid(action: string, ...extras: string[]) {
		const key = process.env.PCAPDROID_API_KEY;
		const auth = key ? ["-e", "api_key", key] : [];
		this.robot.adb("shell", "am", "start", "-e", "action", action, ...auth, ...extras, "-n", PCAPDROID_CTRL);
	}

	private async startApp(app: string): Promise<string> {
		const installed = this.robot.adb("shell", "pm", "list", "packages", PCAPDROID).toString();
		if (!installed.includes(`package:${PCAPDROID}`)) {
			throw new ActionableError("PCAPdroid is not installed (F-Droid: com.emanuelef.remote_capture)");
		}

		this.removeFiles();
		this.dir = fs.mkdtempSync(path.join(os.tmpdir(), "mobile-mcp-net-"));
		this.pcapName = `mobile-mcp-${Date.now()}.pcap`;
		this.pcapdroid("start", "-e", "app_filter", app, "-e", "pcap_dump_mode", "pcap_file", "-e", "pcap_name", this.pcapName);
		await sleep(1500);
		const consent = process.env.PCAPDROID_API_KEY ? "" : " (no PCAPDROID_API_KEY: approve the prompts on the device)";
		return `capturing ${app} with PCAPdroid, hosts only${consent}`;
	}

	// pulls the pcap (also while it is still being written) and summarizes it per host
	private appHosts(filter?: string): string {
		const local = path.join(this.dir, "capture.pcap");
		try {
			this.robot.adb("pull", `${PCAP_DIR}/${this.pcapName || "missing"}`, local);
		} catch (error: any) {
			return "no traffic captured yet";
		}

		const wanted = filter?.toLowerCase();
		const lines = execFileSync("python3", ["-I", PCAP_HOSTS, local]).toString()
			.split("\n")
			.filter(line => line.trim() !== "" && (!wanted || line.toLowerCase().includes(wanted)));
		return lines.length ? lines.slice(0, LIST_LIMIT).join("\n") : "no traffic captured yet";
	}

	public async start(decrypt?: string, app?: string): Promise<string> {
		if (this.proc || this.pcapName) {
			return "already capturing; stop first";
		}

		if (app) {
			return this.startApp(app);
		}

		if (decrypt) {
			try {
				new RegExp(decrypt);
			} catch (error: any) {
				throw new ActionableError(`decrypt is not a valid regex: ${error.message}`);
			}
		}

		this.removeFiles();
		this.dir = fs.mkdtempSync(path.join(os.tmpdir(), "mobile-mcp-net-"));
		this.port = await freePort();
		this.cursor = 0;

		const env = { ...process.env, NET_LOG: this.logFile, NET_DETAIL: this.detailFile, NET_DECRYPT: decrypt || "" };
		this.proc = spawn(findMitmdump(), ["-q", "--listen-host", "127.0.0.1", "-p", `${this.port}`, "-s", ADDON], { env, stdio: "ignore" });
		this.proc.once("exit", code => {
			trace(`mitmdump exited with ${code}`);
			this.proc = null;
			this.clearProxy();
		});

		for (let i = 0; i < 50 && !await isListening(this.port); i++) {
			await sleep(100);
		}

		if (!await isListening(this.port)) {
			this.stopProcess();
			throw new Error("mitmdump did not start");
		}

		this.robot.adb("reverse", `tcp:${this.port}`, `tcp:${this.port}`);
		this.robot.adb("shell", "settings", "put", "global", "http_proxy", `127.0.0.1:${this.port}`);
		return `capturing via 127.0.0.1:${this.port}` + (decrypt ? `, decrypting hosts matching /${decrypt}/` : ", https hosts only (no decrypt)");
	}

	// new requests since the previous list; tls passthrough grouped per host
	public list(filter?: string, all?: boolean): string {
		if (this.pcapName) {
			return this.appHosts(filter);
		}

		const records = readJsonl<Summary>(this.logFile);
		const fresh = all ? records : records.filter(r => r.id > this.cursor);
		if (records.length > 0) {
			this.cursor = records[records.length - 1].id;
		}

		const wanted = filter?.toLowerCase();
		const matching = fresh.filter(r => !wanted || `${r.h}${r.p || ""}`.toLowerCase().includes(wanted));

		const lines: string[] = [];
		const tlsHosts = new Map<string, number>();
		for (const r of matching) {
			if (r.tls) {
				tlsHosts.set(r.h, (tlsHosts.get(r.h) || 0) + 1);
				continue;
			}

			const p = (r.p || "").length > MAX_PATH ? r.p!.substring(0, MAX_PATH - 1) + "…" : r.p;
			lines.push(r.err
				? `#${r.id} ${r.m} ERR ${r.h}${p} ${r.err}`
				: `#${r.id} ${r.m} ${r.s} ${r.h}${p} ${size(r.r)} ${r.ms}ms${r.ct ? " " + r.ct : ""}`);
		}

		const shown = lines.slice(-LIST_LIMIT);
		const out = [...(lines.length > shown.length ? [`(${lines.length - shown.length} older omitted)`] : []), ...shown];
		if (tlsHosts.size > 0) {
			out.push("tls (not decrypted): " + [...tlsHosts].map(([h, n]) => (n > 1 ? `${h}×${n}` : h)).join(", "));
		}

		const state = this.proc ? "" : " (capture stopped)";
		return out.length ? out.join("\n") + state : `no new requests${state}`;
	}

	public show(id: number, all?: boolean): string {
		const detail = readJsonl<any>(this.detailFile).find(d => d.id === id);
		if (!detail) {
			throw new ActionableError(`No details for #${id}; only decrypted or plain http requests have them`);
		}

		const headers = (h: Record<string, string>) => Object.entries(h || {})
			.filter(([k]) => all || !NOISY_HEADER.test(k))
			.map(([k, v]) => `  ${k}: ${v}`)
			.join("\n");
		return [
			detail.url,
			"request headers:", headers(detail.req_headers),
			...(detail.req_body ? ["request body:", detail.req_body] : []),
			...(detail.error ? [`error: ${detail.error}`] : ["response headers:", headers(detail.resp_headers)]),
			...(detail.resp_body ? ["response body:", detail.resp_body] : []),
		].join("\n");
	}

	public stop(): string {
		if (this.pcapName) {
			this.pcapdroid("stop");
			const hosts = this.appHosts();
			this.robot.adb("shell", "rm", "-f", `${PCAP_DIR}/${this.pcapName}`);
			this.pcapName = "";
			return `stopped PCAPdroid, capture file removed from the device\n${hosts}`;
		}

		const total = readJsonl<Summary>(this.logFile).length;
		this.stopProcess();
		return `stopped, proxy cleared. ${total} records kept until the next start`;
	}

	private stopProcess() {
		this.clearProxy();
		this.proc?.kill();
		this.proc = null;
	}

	// leaving the proxy set after mitmdump is gone cuts the device off the internet
	public clearProxy() {
		try {
			this.robot.adb("shell", "settings", "put", "global", "http_proxy", ":0");
			this.robot.adb("shell", "settings", "delete", "global", "http_proxy");
			if (this.port) {
				this.robot.adb("reverse", "--remove", `tcp:${this.port}`);
			}
		} catch (error: any) {
			trace(`failed to clear proxy: ${error.message}`);
		}
	}

	public removeFiles() {
		if (this.dir) {
			fs.rmSync(this.dir, { recursive: true, force: true });
			this.dir = "";
		}
	}
}
