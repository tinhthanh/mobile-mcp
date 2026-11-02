import { McpServer } from "@modelcontextprotocol/server";
import { z } from "zod";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";

import { trace } from "./logger";
import { AndroidRobot, AndroidDeviceManager } from "./android";
import { ActionableError, Button } from "./robot";
import { compactScreen, findLine, SCREEN_LEGEND } from "./screen";
import { ocrLines } from "./ocr";
import { validatePackageName } from "./utils";
import { Uia2Client } from "./uia2";
import { NetCapture } from "./net";

// below this many lines the accessibility tree is probably useless (game, canvas, flutter)
const OCR_FALLBACK_MIN_LINES = 3;
const SETTLE_MS = 500;
// with the uia2 server a read is ~0.4s, cheap enough to poll until two reads agree
const STABLE_MIN_MS = 300;
const STABLE_MAX_MS = 4000;
const SCREENSHOT_MAX_SIZE = 1024;

const BUTTONS = ["BACK", "HOME", "ENTER", "VOLUME_UP", "VOLUME_DOWN", "DPAD_CENTER", "DPAD_UP", "DPAD_DOWN", "DPAD_LEFT", "DPAD_RIGHT"] as const;

export const getAgentVersion = (): string => {
	const json = require("../package.json");
	return json.version;
};

export const getSdkVersion = (): string => {
	try {
		// the sdk doesn't export its package.json, so resolve the installed
		// package directory and read it from disk.
		const entry = require.resolve("@modelcontextprotocol/server");
		const packageRoot = entry.slice(0, entry.lastIndexOf(`${path.sep}dist${path.sep}`));
		return JSON.parse(fs.readFileSync(path.join(packageRoot, "package.json"), "utf8")).version;
	} catch (err: any) {
		return "unknown version";
	}
};

const SERVER_INSTRUCTIONS = `Drive an Android device over adb.
Call devices once, then pass the device id to every call.
Read the screen with screen (cheap text). ${SCREEN_LEGEND}
tap, type, swipe, key and app already return the new screen, or "unchanged" - do not call screen again after them.
Use screenshot only to judge visual appearance.`;

const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

// sips ships with macOS; elsewhere the full-size png is returned
const shrinkToJpeg = (png: Buffer): { data: Buffer, mimeType: string } => {
	if (process.platform !== "darwin") {
		return { data: png, mimeType: "image/png" };
	}

	const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mobile-mcp-shot-"));
	try {
		const src = path.join(dir, "in.png"), dst = path.join(dir, "out.jpg");
		fs.writeFileSync(src, png);
		execFileSync("sips", ["-Z", `${SCREENSHOT_MAX_SIZE}`, "-s", "format", "jpeg", "-s", "formatOptions", "70", src, "--out", dst], { stdio: "pipe" });
		return { data: fs.readFileSync(dst), mimeType: "image/jpeg" };
	} catch (err: any) {
		trace(`sips failed, returning png: ${err.message}`);
		return { data: png, mimeType: "image/png" };
	} finally {
		fs.rmSync(dir, { recursive: true, force: true });
	}
};

// module level: the http transport builds a server per request, devices outlive that
const robots = new Map<string, AndroidRobot>();
const uia2Clients = new Map<string, Uia2Client | null>();
const lastScreen = new Map<string, string>();
const captures = new Map<string, NetCapture>();

process.on("exit", () => {
	for (const client of uia2Clients.values()) {
		client?.stop();
	}

	for (const capture of captures.values()) {
		if (capture.running) {
			capture.stop();
		}

		capture.removeFiles();
	}
});

const getRobot = (deviceId: string): AndroidRobot => {
	let robot = robots.get(deviceId);
	if (!robot) {
		const devices = new AndroidDeviceManager().getConnectedDevices();
		if (!devices.find(d => d.deviceId === deviceId)) {
			throw new ActionableError(`Device "${deviceId}" not found. Use the devices tool to see available devices`);
		}

		robot = new AndroidRobot(deviceId);
		robots.set(deviceId, robot);
		NetCapture.clearStaleProxy(robot).catch(err => trace(`stale proxy check failed: ${err.message}`));
	}

	return robot;
};

const getUia2 = (deviceId: string): Uia2Client | null => {
	if (!uia2Clients.has(deviceId)) {
		const robot = getRobot(deviceId);
		const usable = !process.env.MOBILEMCP_DISABLE_UIA2 && Uia2Client.isInstalled(robot);
		uia2Clients.set(deviceId, usable ? new Uia2Client(robot, deviceId) : null);
	}

	return uia2Clients.get(deviceId)!;
};

export const createMcpServer = (): McpServer => {

	const server = new McpServer({
		name: "mobile-mcp-lite",
		version: getAgentVersion(),
	}, {
		instructions: SERVER_INSTRUCTIONS,
	});

	type ZodSchemaShape = Record<string, z.ZodType>;

	interface ToolAnnotations {
		readOnlyHint?: boolean;
		destructiveHint?: boolean;
	}

	type ToolResult = string | { type: "image", data: string, mimeType: string };

	const tool = (name: string, description: string, paramsSchema: ZodSchemaShape, annotations: ToolAnnotations, cb: (args: any) => Promise<ToolResult>) => {
		server.registerTool(name, {
			description,
			inputSchema: z.object(paramsSchema),
			annotations,
		}, (async (args: any) => {
			try {
				trace(`Invoking ${name} with args: ${JSON.stringify(args)}`);
				const response = await cb(args);
				if (typeof response === "string") {
					trace(`=> ${response}`);
					return { content: [{ type: "text", text: response }] };
				}

				return { content: [response] };
			} catch (error: any) {
				if (error instanceof ActionableError) {
					return { content: [{ type: "text", text: `${error.message}. Please fix the issue and try again.` }], isError: true };
				}

				trace(`Tool '${name}' failed: ${error.message} stack: ${error.stack}`);
				return { content: [{ type: "text", text: `Error: ${error.message}` }], isError: true };
			}
		}) as any);
	};

	const captureScreen = async (deviceId: string, forceOcr: boolean): Promise<string> => {
		const robot = getRobot(deviceId);
		const source = await getUia2(deviceId)?.getSource() ?? await robot.getUiAutomatorDump();
		const { app, lines } = compactScreen(source);

		let body = lines;
		let via = "";
		if (forceOcr || lines.length < OCR_FALLBACK_MIN_LINES) {
			const ocr = ocrLines(await robot.getScreenshot());
			if (ocr !== null) {
				body = forceOcr ? ocr : [...lines, ...ocr];
				via = " (ocr)";
			}
		}

		return [`app=${app}${via}`, ...body].join("\n");
	};

	// returns "unchanged" when the screen is what the agent saw last
	const report = (deviceId: string, text: string): string => {
		if (lastScreen.get(deviceId) === text) {
			return "unchanged";
		}

		lastScreen.set(deviceId, text);
		return text;
	};

	// waits for the ui to settle so the agent does not get a half-drawn transition
	const settledScreen = async (deviceId: string): Promise<string> => {
		if (!getUia2(deviceId)) {
			await sleep(SETTLE_MS);
			return captureScreen(deviceId, false);
		}

		await sleep(STABLE_MIN_MS);
		const deadline = Date.now() + STABLE_MAX_MS;
		let previous = await captureScreen(deviceId, false);
		while (Date.now() < deadline) {
			const current = await captureScreen(deviceId, false);
			if (current === previous) {
				break;
			}

			previous = current;
		}

		return previous;
	};

	const afterAction = async (deviceId: string, observe: boolean, prefix = ""): Promise<string> => {
		if (!observe) {
			lastScreen.delete(deviceId);
			return prefix + "ok";
		}

		return prefix + report(deviceId, await settledScreen(deviceId));
	};

	const device = z.string().describe("Device id from devices");
	const observe = z.boolean().optional().describe("Return the new screen afterwards (default true)");

	tool(
		"devices",
		"List connected Android devices (id model android-version).",
		{},
		{ readOnlyHint: true },
		async () => {
			const devices = new AndroidDeviceManager().getConnectedDevicesWithDetails();
			if (devices.length === 0) {
				return "No device. Connect one over USB with USB debugging enabled and authorized.";
			}

			return devices.map(d => `${d.deviceId} ${d.name.replace(/\s+/g, "_")} android${d.version}`).join("\n");
		}
	);

	tool(
		"screen",
		"Read the current screen as compact text. Falls back to OCR when the accessibility tree is nearly empty; set ocr=true to force OCR (games, canvas, webviews).",
		{
			device,
			ocr: z.boolean().optional().describe("Read text with OCR instead of the accessibility tree"),
		},
		{ readOnlyHint: true },
		async ({ device, ocr }) => {
			lastScreen.delete(device);
			return report(device, await captureScreen(device, !!ocr));
		}
	);

	tool(
		"tap",
		"Tap at x,y from a screen line, or on the element whose label matches text (one call instead of screen + tap).",
		{
			device,
			x: z.number().int().optional(),
			y: z.number().int().optional(),
			text: z.string().optional().describe("Label to find on the current screen, used when x,y are not given"),
			mode: z.enum(["tap", "double", "long"]).optional(),
			observe,
		},
		{ destructiveHint: true },
		async ({ device, x, y, text, mode, observe }) => {
			const robot = getRobot(device);
			let prefix = "";
			if (x === undefined || y === undefined) {
				if (!text) {
					throw new ActionableError("Give x,y or text");
				}

				const screen = await captureScreen(device, false);
				const found = findLine(screen, text);
				if (!found) {
					lastScreen.set(device, screen);
					throw new ActionableError(`No element matches "${text}". Current screen:\n${screen}\n`);
				}

				({ x, y } = found);
				prefix = `tapped ${found.line}` + (found.matches > 1 ? ` (first of ${found.matches} matches)` : "") + "\n";
			}

			if (mode === "double") {
				await robot.doubleTap(x, y);
			} else if (mode === "long") {
				await robot.longPress(x, y, 800);
			} else {
				await robot.tap(x, y);
			}

			return afterAction(device, observe !== false, prefix);
		}
	);

	tool(
		"type",
		"Set the focused input's text (replaces it, unicode ok). Tap the input first. submit=true presses ENTER after.",
		{
			device,
			text: z.string(),
			submit: z.boolean().optional(),
			observe,
		},
		{ destructiveHint: true },
		async ({ device, text, submit, observe }) => {
			const robot = getRobot(device);
			// uia2 setText handles unicode and replaces the field; adb `input text` is ascii only and appends
			const typed = await getUia2(device)?.setFocusedText(text);
			if (!typed) {
				await robot.sendKeys(text);
			}

			if (submit) {
				await robot.pressButton("ENTER");
			}

			return afterAction(device, observe !== false);
		}
	);

	tool(
		"swipe",
		"Swipe to scroll. Without x,y swipes across the screen center. direction=up scrolls content down.",
		{
			device,
			direction: z.enum(["up", "down", "left", "right"]),
			x: z.number().int().optional(),
			y: z.number().int().optional(),
			distance: z.number().int().optional().describe("Pixels, only with x,y"),
			observe,
		},
		{ destructiveHint: true },
		async ({ device, direction, x, y, distance, observe }) => {
			const robot = getRobot(device);
			if (x !== undefined && y !== undefined) {
				await robot.swipeFromCoordinate(x, y, direction, distance);
			} else {
				await robot.swipe(direction);
			}

			return afterAction(device, observe !== false);
		}
	);

	tool(
		"key",
		"Press a hardware/navigation button.",
		{
			device,
			button: z.enum(BUTTONS),
			observe,
		},
		{ destructiveHint: true },
		async ({ device, button, observe }) => {
			await getRobot(device).pressButton(button as Button);
			return afterAction(device, observe !== false);
		}
	);

	tool(
		"app",
		"launch or stop an app by package name, or list launchable packages.",
		{
			device,
			action: z.enum(["launch", "stop", "list"]),
			package: z.string().optional(),
			observe,
		},
		{ destructiveHint: true },
		async ({ device, action, package: pkg, observe }) => {
			const robot = getRobot(device);
			if (action === "list") {
				return (await robot.listApps()).map(a => a.packageName).join("\n");
			}

			if (!pkg) {
				throw new ActionableError(`package is required for ${action}`);
			}

			validatePackageName(pkg);
			if (action === "launch") {
				await robot.launchApp(pkg);
			} else {
				await robot.terminateApp(pkg);
			}

			return afterAction(device, observe !== false);
		}
	);

	tool(
		"net",
		"Capture the device's HTTP(S) requests through mitmproxy over USB. start, use the app, then list (only new requests since the last list) and show {id} for headers/bodies; always stop when done or the device keeps a dead proxy. " +
			"Only apps honoring the system proxy appear. HTTPS shows hosts only unless decrypt matches the host and the app trusts the mitmproxy CA (debug builds with network_security_config).",
		{
			device,
			action: z.enum(["start", "list", "show", "stop"]),
			filter: z.string().optional().describe("list: substring of host or path"),
			all: z.boolean().optional().describe("list: include requests already listed; show: keep browser/transport headers"),
			id: z.number().int().optional().describe("show: request id from list"),
			decrypt: z.string().optional().describe("start: regex of hosts to decrypt, e.g. api\\.myapp\\.com"),
			app: z.string().optional().describe("start: package to capture with PCAPdroid instead (sees apps that bypass the proxy, hosts only)"),
		},
		{ destructiveHint: true },
		async ({ device, action, filter, all, id, decrypt, app }) => {
			let capture = captures.get(device);
			if (!capture) {
				capture = new NetCapture(getRobot(device));
				captures.set(device, capture);
			}

			switch (action) {
				case "start":
					if (app) {
						validatePackageName(app);
					}

					return capture.start(decrypt, app);
				case "list":
					return capture.list(filter, all);
				case "show":
					if (id === undefined) {
						throw new ActionableError("id is required for show");
					}

					return capture.show(id, all);
				default:
					return capture.stop();
			}
		}
	);

	tool(
		"screenshot",
		"Screenshot as a downscaled JPEG. Costs far more tokens than screen; use only for visual checks.",
		{ device },
		{ readOnlyHint: true },
		async ({ device }) => {
			const { data, mimeType } = shrinkToJpeg(await getRobot(device).getScreenshot());
			return { type: "image", data: data.toString("base64"), mimeType };
		}
	);

	return server;
};
