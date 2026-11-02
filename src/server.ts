import { McpServer } from "@modelcontextprotocol/server";
import { z } from "zod";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";

import { trace } from "./logger";
import { AndroidRobot, AndroidDeviceManager, getAdbPath } from "./android";
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
const POLL_MS = 300;
const DEFAULT_WAIT_MS = 10000;
const MAX_WAIT_MS = 30000;
const MAX_STEPS = 30;
const DEVICE_CACHE_MS = 10000;

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

const SERVER_INSTRUCTIONS = `Drive an Android device over adb. device may be omitted when only one is connected.
Read the screen with screen. ${SCREEN_LEGEND}
tap, type, swipe, key, app and steps return the new screen (or "unchanged"); observe=false skips it. Do not call screen right after them.
Batch known sequences with steps; use wait instead of polling screen.`;

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

let cachedDevices: { at: number, ids: string[] } | null = null;

// the device param is optional: with a single connected device there is nothing to choose
const resolveDevice = (deviceId?: string): string => {
	if (deviceId) {
		return deviceId;
	}

	if (!cachedDevices || Date.now() - cachedDevices.at > DEVICE_CACHE_MS) {
		const ids = execFileSync(getAdbPath(), ["devices"]).toString()
			.split("\n")
			.map(line => line.trim().split("\t"))
			.filter(([, state]) => state === "device")
			.map(([id]) => id);
		cachedDevices = { at: Date.now(), ids };
	}

	if (cachedDevices.ids.length !== 1) {
		const found = cachedDevices.ids.length ? cachedDevices.ids.join(", ") : "none";
		throw new ActionableError(`Pass device: ${cachedDevices.ids.length} devices connected (${found})`);
	}

	return cachedDevices.ids[0];
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

	const device = z.string().optional();
	const observe = z.boolean().optional();

	const tap = async (deviceId: string, x: number | undefined, y: number | undefined, text?: string, mode?: string, waitMs = 0): Promise<string> => {
		const robot = getRobot(deviceId);
		let prefix = "";
		if (x === undefined || y === undefined) {
			if (!text) {
				throw new ActionableError("Give x,y or text");
			}

			// the target may still be animating in, so look for it a little while
			const deadline = Date.now() + waitMs;
			let screen = await captureScreen(deviceId, false);
			let found = findLine(screen, text);
			while (!found && Date.now() < deadline) {
				await sleep(POLL_MS);
				screen = await captureScreen(deviceId, false);
				found = findLine(screen, text);
			}

			if (!found) {
				lastScreen.set(deviceId, screen);
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

		return prefix;
	};

	const type = async (deviceId: string, text: string, submit?: boolean) => {
		// uia2 setText handles unicode and replaces the field; adb `input text` is ascii only and appends
		const typed = await getUia2(deviceId)?.setFocusedText(text);
		if (!typed) {
			await getRobot(deviceId).sendKeys(text);
		}

		if (submit) {
			await getRobot(deviceId).pressButton("ENTER");
		}
	};

	const app = async (deviceId: string, action: string, pkg?: string) => {
		if (!pkg) {
			throw new ActionableError(`package is required for ${action}`);
		}

		validatePackageName(pkg);
		if (action === "launch") {
			await getRobot(deviceId).launchApp(pkg);
		} else {
			await getRobot(deviceId).terminateApp(pkg);
		}
	};

	// polls until text shows up (or is gone), returning the screen it matched on
	const waitFor = async (deviceId: string, text: string, gone: boolean, timeoutMs: number): Promise<string> => {
		const deadline = Date.now() + Math.min(timeoutMs, MAX_WAIT_MS);
		for (;;) {
			const screen = await captureScreen(deviceId, false);
			if (!!findLine(screen, text) !== gone) {
				return screen;
			}

			if (Date.now() >= deadline) {
				lastScreen.set(deviceId, screen);
				throw new ActionableError(`Timed out waiting for "${text}" to ${gone ? "disappear" : "appear"}. Current screen:\n${screen}\n`);
			}

			await sleep(POLL_MS);
		}
	};

	tool(
		"devices",
		"List connected devices: id model android-version.",
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
		"Read the screen as compact text. ocr=true reads text by OCR (games, canvas); image=true returns a JPEG instead, costing far more tokens.",
		{ device, ocr: z.boolean().optional(), image: z.boolean().optional() },
		{ readOnlyHint: true },
		async args => {
			const id = resolveDevice(args.device);
			if (args.image) {
				const { data, mimeType } = shrinkToJpeg(await getRobot(id).getScreenshot());
				return { type: "image", data: data.toString("base64"), mimeType };
			}

			lastScreen.delete(id);
			return report(id, await captureScreen(id, !!args.ocr));
		}
	);

	tool(
		"tap",
		"Tap x,y from a screen line, or the element whose label matches text.",
		{ device, x: z.number().int().optional(), y: z.number().int().optional(), text: z.string().optional(), mode: z.enum(["tap", "double", "long"]).optional(), observe },
		{ destructiveHint: true },
		async args => {
			const id = resolveDevice(args.device);
			const prefix = await tap(id, args.x, args.y, args.text, args.mode);
			return afterAction(id, args.observe !== false, prefix);
		}
	);

	tool(
		"type",
		"Set the focused input's text (replaces it, unicode ok). submit presses ENTER.",
		{ device, text: z.string(), submit: z.boolean().optional(), observe },
		{ destructiveHint: true },
		async args => {
			const id = resolveDevice(args.device);
			await type(id, args.text, args.submit);
			return afterAction(id, args.observe !== false);
		}
	);

	tool(
		"swipe",
		"Swipe; up scrolls content down. Optional start x,y and distance in px.",
		{ device, direction: z.enum(["up", "down", "left", "right"]), x: z.number().int().optional(), y: z.number().int().optional(), distance: z.number().int().optional(), observe },
		{ destructiveHint: true },
		async args => {
			const id = resolveDevice(args.device);
			const robot = getRobot(id);
			if (args.x !== undefined && args.y !== undefined) {
				await robot.swipeFromCoordinate(args.x, args.y, args.direction, args.distance);
			} else {
				await robot.swipe(args.direction);
			}

			return afterAction(id, args.observe !== false);
		}
	);

	tool(
		"key",
		"Press a button.",
		{ device, button: z.enum(BUTTONS), observe },
		{ destructiveHint: true },
		async args => {
			const id = resolveDevice(args.device);
			await getRobot(id).pressButton(args.button as Button);
			return afterAction(id, args.observe !== false);
		}
	);

	tool(
		"app",
		"Launch or stop a package, or list launchable packages.",
		{ device, action: z.enum(["launch", "stop", "list"]), package: z.string().optional(), observe },
		{ destructiveHint: true },
		async args => {
			const id = resolveDevice(args.device);
			if (args.action === "list") {
				return (await getRobot(id).listApps()).map(a => a.packageName).join("\n");
			}

			await app(id, args.action, args.package);
			return afterAction(id, args.observe !== false);
		}
	);

	tool(
		"wait",
		"Wait until text appears (or is gone) on screen, then return the screen.",
		{ device, text: z.string(), gone: z.boolean().optional(), timeout: z.number().int().optional().describe("ms, default 10000") },
		{ readOnlyHint: true },
		async args => {
			const id = resolveDevice(args.device);
			return report(id, await waitFor(id, args.text, !!args.gone, args.timeout ?? DEFAULT_WAIT_MS));
		}
	);

	tool(
		"steps",
		"Run actions in order, then return the final screen; stops at the first failure. " +
			"Each item: \"tap <text|x,y>\", \"long <text|x,y>\", \"type <text>\", \"enter\", \"key <BUTTON>\", \"swipe <up|down|left|right>\", " +
			"\"wait <text>\", \"gone <text>\", \"launch <package>\", \"stop <package>\", \"sleep <ms>\".",
		{ device, steps: z.array(z.string()).min(1).max(MAX_STEPS) },
		{ destructiveHint: true },
		async args => {
			const id = resolveDevice(args.device);
			const notes: string[] = [];
			for (let i = 0; i < args.steps.length; i++) {
				const stepText = args.steps[i].trim();
				const space = stepText.indexOf(" ");
				const verb = (space < 0 ? stepText : stepText.substring(0, space)).toLowerCase();
				const rest = space < 0 ? "" : stepText.substring(space + 1).trim();
				const point = rest.match(/^(\d+)\s*,\s*(\d+)$/);
				try {
					// let the previous action finish its transition before acting on coordinates
					if (i > 0 && (point || verb === "type" || verb === "enter")) {
						await settledScreen(id);
					}

					switch (verb) {
						case "tap":
						case "long": {
							const note = point
								? await tap(id, Number(point[1]), Number(point[2]), undefined, verb)
								: await tap(id, undefined, undefined, rest, verb, DEFAULT_WAIT_MS);
							if (note) {
								notes.push(note.trim());
							}

							break;
						}

						case "type":
							await type(id, rest);
							break;
						case "enter":
							await getRobot(id).pressButton("ENTER");
							break;
						case "key":
							await getRobot(id).pressButton(rest.toUpperCase() as Button);
							break;
						case "swipe":
							await getRobot(id).swipe(rest.toLowerCase() as any);
							break;
						case "wait":
						case "gone":
							await waitFor(id, rest, verb === "gone", DEFAULT_WAIT_MS);
							break;
						case "launch":
						case "stop":
							await app(id, verb, rest);
							break;
						case "sleep":
							await sleep(Math.min(Number(rest) || 0, MAX_WAIT_MS));
							break;
						default:
							throw new ActionableError(`unknown action "${verb}"`);
					}
				} catch (error: any) {
					const reason = error.message.split("\n")[0];
					const screen = await captureScreen(id, false);
					lastScreen.set(id, screen);
					throw new ActionableError(`step ${i + 1} "${stepText}" failed: ${reason}\n${[...notes, screen].join("\n")}\n`);
				}
			}

			return afterAction(id, true, [`done ${args.steps.length} steps`, ...notes].join("\n") + "\n");
		}
	);

	tool(
		"net",
		"Capture HTTP(S) requests. start, use the app, list (new since last list), show {id} for headers/bodies, stop when done. " +
			"Default: mitmproxy over USB for apps using the system proxy; HTTPS hosts only unless decrypt matches and the app trusts the user CA. " +
			"start {app} uses PCAPdroid instead: every connection of that package, hosts only.",
		{
			device,
			action: z.enum(["start", "list", "show", "stop"]),
			filter: z.string().optional(),
			all: z.boolean().optional().describe("list: include already listed; show: all headers"),
			id: z.number().int().optional(),
			decrypt: z.string().optional().describe("host regex"),
			app: z.string().optional().describe("package"),
		},
		{ destructiveHint: true },
		async args => {
			const id = resolveDevice(args.device);
			let capture = captures.get(id);
			if (!capture) {
				capture = new NetCapture(getRobot(id));
				captures.set(id, capture);
			}

			switch (args.action) {
				case "start":
					if (args.app) {
						validatePackageName(args.app);
					}

					return capture.start(args.decrypt, args.app);
				case "list":
					return capture.list(args.filter, args.all);
				case "show":
					if (args.id === undefined) {
						throw new ActionableError("id is required for show");
					}

					return capture.show(args.id, args.all);
				default:
					return capture.stop();
			}
		}
	);

	return server;
};
