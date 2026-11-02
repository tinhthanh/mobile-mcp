import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { test, expect } from "@playwright/test";

import { NetCapture } from "../src/net";

const NATIVE = path.join(__dirname, "..", "native");

const tempDir = () => fs.mkdtempSync(path.join(os.tmpdir(), "mobile-mcp-net-test-"));

// records adb calls and answers `settings get` from a map
const fakeRobot = (deviceId: string, settings: Record<string, string> = {}) => {
	const calls: string[][] = [];
	return {
		calls,
		robot: {
			deviceId,
			adb: (...args: string[]) => {
				calls.push(args);
				const isGet = args[0] === "shell" && args[1] === "settings" && args[2] === "get";
				return Buffer.from(isGet ? (settings[args[4]] ?? "null") + "\n" : "");
			},
		} as any,
	};
};

test.describe("net redaction", () => {

	const redact = (text: string): string => execFileSync("python3", ["-I", "-c",
		"import sys; sys.path.insert(0, sys.argv[1]); from net_redact import redact; sys.stdout.write(redact(sys.stdin.read()))",
		NATIVE], { input: text }).toString();

	test("hides credential values in json, escaped json, forms and bearer tokens", async () => {
		const out = redact([
			`{"access_token": "abc.def", "user": "an", "password": "p\\"q"}`,
			`{"data": "{\\"password\\":\\"hunter2\\"}"}`,
			`user=an&refresh_token=zzz&page=2`,
			`Authorization: Bearer eyJhbGciOi.xyz`,
		].join("\n"));

		for (const secret of ["abc.def", "p\\\"q", "hunter2", "zzz", "eyJhbGciOi"]) {
			expect(out).not.toContain(secret);
		}

		expect(out).toContain(`"user": "an"`);
		expect(out).toContain("page=2");
	});
});

test.describe("pcap host summary", () => {

	const u16 = (n: number) => Buffer.from([n >> 8, n & 0xff]);
	const u24 = (n: number) => Buffer.from([n >> 16, (n >> 8) & 0xff, n & 0xff]);
	const ip = (s: string) => Buffer.from(s.split(".").map(Number));

	const ipv4 = (proto: number, src: string, dst: string, l4: Buffer) => Buffer.concat([
		Buffer.from([0x45, 0]), u16(20 + l4.length), Buffer.from([0, 0, 0, 0, 64, proto, 0, 0]), ip(src), ip(dst), l4,
	]);
	const tcp = (sport: number, dport: number, payload: Buffer) => Buffer.concat([
		u16(sport), u16(dport), Buffer.alloc(8), Buffer.from([5 << 4, 0x18]), u16(65535), Buffer.alloc(4), payload,
	]);
	const udp = (sport: number, dport: number, payload: Buffer) => Buffer.concat([u16(sport), u16(dport), u16(8 + payload.length), u16(0), payload]);

	const clientHello = (sni: string) => {
		const name = Buffer.from(sni);
		const serverName = Buffer.concat([u16(name.length + 3), Buffer.from([0]), u16(name.length), name]);
		const extensions = Buffer.concat([u16(0), u16(serverName.length), serverName]);
		const body = Buffer.concat([u16(0x0303), Buffer.alloc(32), Buffer.from([0]), u16(2), u16(0x1301), Buffer.from([1, 0]), u16(extensions.length), extensions]);
		const handshake = Buffer.concat([Buffer.from([1]), u24(body.length), body]);
		return Buffer.concat([Buffer.from([0x16, 3, 1]), u16(handshake.length), handshake]);
	};

	const dnsAnswer = (name: string, address: string) => {
		const qname = Buffer.concat([...name.split(".").map(l => Buffer.concat([Buffer.from([l.length]), Buffer.from(l)])), Buffer.from([0])]);
		return Buffer.concat([u16(7), u16(0x8180), u16(1), u16(1), u16(0), u16(0), qname, u16(1), u16(1),
			u16(0xc00c), u16(1), u16(1), Buffer.from([0, 0, 0, 60]), u16(4), ip(address)]);
	};

	const pcap = (packets: Buffer[]) => {
		const header = Buffer.alloc(24);
		header.writeUInt32LE(0xa1b2c3d4, 0);
		header.writeUInt16LE(2, 4);
		header.writeUInt16LE(4, 6);
		header.writeUInt32LE(65535, 16);
		header.writeUInt32LE(101, 20); // raw ip, like pcapdroid's vpn mode
		return Buffer.concat([header, ...packets.flatMap(p => {
			const rec = Buffer.alloc(16);
			rec.writeUInt32LE(p.length, 8);
			rec.writeUInt32LE(p.length, 12);
			return [rec, p];
		})]);
	};

	test("names destinations from tls sni, http host and dns answers", async () => {
		const app = "10.215.173.1";
		const file = path.join(tempDir(), "capture.pcap");
		fs.writeFileSync(file, pcap([
			ipv4(17, "10.215.173.2", app, udp(53, 40000, dnsAnswer("cdn.example.com", "203.0.113.9"))),
			ipv4(6, app, "198.51.100.1", tcp(40001, 443, clientHello("api.example.com"))),
			ipv4(6, "198.51.100.1", app, tcp(443, 40001, Buffer.alloc(3000))),
			ipv4(6, app, "198.51.100.2", tcp(40002, 80, Buffer.from("GET / HTTP/1.1\r\nHost: plain.example.com\r\n\r\n"))),
			ipv4(6, app, "203.0.113.9", tcp(40003, 443, Buffer.alloc(10))),
		]));

		const lines = execFileSync("python3", ["-I", path.join(NATIVE, "pcap_hosts.py"), file]).toString().trim().split("\n");
		expect(lines[0]).toMatch(/^tls api\.example\.com:443 conns=1 bytes=\d+$/);
		expect(lines).toContainEqual(expect.stringMatching(/^http plain\.example\.com:80 conns=1 /));
		expect(lines).toContainEqual(expect.stringMatching(/^tls cdn\.example\.com:443 conns=1 bytes=10$/));
	});
});

test.describe("net capture", () => {

	const withLogs = (summaries: object[], details: object[] = []) => {
		const { robot } = fakeRobot("SERIAL1");
		const capture = new NetCapture(robot);
		const dir = tempDir();
		fs.writeFileSync(path.join(dir, "log.jsonl"), summaries.map(s => JSON.stringify(s)).join("\n") + "\n");
		fs.writeFileSync(path.join(dir, "detail.jsonl"), details.map(s => JSON.stringify(s)).join("\n") + "\n");
		(capture as any).dir = dir;
		return { capture, dir };
	};

	test("lists only new requests, groups tls hosts and filters", async () => {
		const { capture, dir } = withLogs([
			{ id: 1, t: 0, m: "GET", s: 200, h: "api.example.com", p: "/v1/users", r: 1300, ms: 140, ct: "application/json" },
			{ id: 2, t: 0, tls: true, h: "cdn.example.com" },
			{ id: 3, t: 0, tls: true, h: "cdn.example.com" },
			{ id: 4, t: 0, m: "POST", s: 0, h: "api.example.com", p: "/v1/login", err: "connection reset" },
		]);

		expect(capture.list()).toBe([
			"#1 GET 200 api.example.com/v1/users 1.3KB 140ms application/json",
			"#4 POST ERR api.example.com/v1/login connection reset",
			"tls (not decrypted): cdn.example.com×2",
		].join("\n") + " (capture stopped)");
		expect(capture.list()).toBe("no new requests (capture stopped)");

		fs.appendFileSync(path.join(dir, "log.jsonl"), JSON.stringify({ id: 5, t: 0, m: "GET", s: 304, h: "img.example.com", p: "/a.png", r: 0, ms: 9 }) + "\n");
		expect(capture.list()).toBe("#5 GET 304 img.example.com/a.png 0 9ms (capture stopped)");
		expect(capture.list("users", true)).toBe("#1 GET 200 api.example.com/v1/users 1.3KB 140ms application/json (capture stopped)");
	});

	test("show drops browser noise headers unless all", async () => {
		const { capture } = withLogs([], [{
			id: 1,
			url: "https://api.example.com/v1/users",
			req_headers: { "sec-fetch-mode": "cors", "accept": "*/*", "authorization": "[redacted]" },
			req_body: "",
			resp_headers: { "content-type": "application/json", "date": "today" },
			resp_body: "{}",
		}]);

		const short = capture.show(1);
		expect(short).toContain("authorization: [redacted]");
		expect(short).toContain("content-type: application/json");
		expect(short).not.toContain("sec-fetch-mode");
		expect(short).not.toContain("date:");
		expect(capture.show(1, true)).toContain("sec-fetch-mode: cors");
	});

	test("reads the per-device api key from the environment, then the keys file", async () => {
		const file = path.join(tempDir(), "keys.env");
		fs.writeFileSync(file, "# comment\nPCAPDROID_API_KEY_OTHER=nope\nPCAPDROID_API_KEY_ABC_5555=from-file\n");
		process.env.MOBILEMCP_KEYS_FILE = file;
		try {
			const capture = new NetCapture(fakeRobot("ABC:5555").robot);
			expect((capture as any).apiKey()).toBe("from-file");
			process.env.PCAPDROID_API_KEY_ABC_5555 = "from-env";
			expect((capture as any).apiKey()).toBe("from-env");
			expect((new NetCapture(fakeRobot("XYZ").robot) as any).apiKey()).toBeUndefined();
		} finally {
			delete process.env.MOBILEMCP_KEYS_FILE;
			delete process.env.PCAPDROID_API_KEY_ABC_5555;
		}
	});

	test("clears a proxy it left behind, but not one the user set", async () => {
		const stale = fakeRobot("S1", { http_proxy: "127.0.0.1:1", mobile_mcp_proxy_port: "1" });
		expect(await NetCapture.clearStaleProxy(stale.robot)).toBe(true);
		expect(stale.calls).toContainEqual(["shell", "settings", "put", "global", "http_proxy", ":0"]);
		expect(stale.calls).toContainEqual(["reverse", "--remove", "tcp:1"]);

		const user = fakeRobot("S2", { http_proxy: "127.0.0.1:8118" });
		expect(await NetCapture.clearStaleProxy(user.robot)).toBe(false);
		expect(user.calls.filter(c => c[2] === "put")).toEqual([]);
	});
});
