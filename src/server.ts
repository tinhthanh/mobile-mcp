import { McpServer } from "@modelcontextprotocol/server";
import { z } from "zod";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";

import { trace } from "./logger";
import { AndroidRobot, AndroidDeviceManager } from "./android";
import { ActionableError, Button } from "./robot";
import { compactScreen, SCREEN_LEGEND } from "./screen";
import { ocrLines } from "./ocr";
import { validatePackageName } from "./utils";

// below this many lines the accessibility tree is probably useless (game, canvas, flutter)
const OCR_FALLBACK_MIN_LINES = 3;
const SETTLE_MS = 500;
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

	const robots = new Map<string, AndroidRobot>();
	const lastScreen = new Map<string, string>();

	const getRobot = (deviceId: string): AndroidRobot => {
		let robot = robots.get(deviceId);
		if (!robot) {
			const devices = new AndroidDeviceManager().getConnectedDevices();
			if (!devices.find(d => d.deviceId === deviceId)) {
				throw new ActionableError(`Device "${deviceId}" not found. Use the devices tool to see available devices`);
			}

			robot = new AndroidRobot(deviceId);
			robots.set(deviceId, robot);
		}

		return robot;
	};

	const readScreen = async (deviceId: string, forceOcr: boolean): Promise<string> => {
		const robot = getRobot(deviceId);
		const { app, lines } = compactScreen(await robot.getUiAutomatorDump());

		let body = lines;
		let source = "";
		if (forceOcr || lines.length < OCR_FALLBACK_MIN_LINES) {
			const ocr = ocrLines(await robot.getScreenshot());
			if (ocr !== null) {
				body = forceOcr ? ocr : [...lines, ...ocr];
				source = " (ocr)";
			}
		}

		const text = [`app=${app}${source}`, ...body].join("\n");
		if (lastScreen.get(deviceId) === text) {
			return "unchanged";
		}

		lastScreen.set(deviceId, text);
		return text;
	};

	const afterAction = async (deviceId: string, observe: boolean): Promise<string> => {
		if (!observe) {
			lastScreen.delete(deviceId);
			return "ok";
		}

		await sleep(SETTLE_MS);
		return readScreen(deviceId, false);
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
			return readScreen(device, !!ocr);
		}
	);

	tool(
		"tap",
		"Tap at x,y taken from a screen line.",
		{
			device,
			x: z.number().int(),
			y: z.number().int(),
			mode: z.enum(["tap", "double", "long"]).optional(),
			observe,
		},
		{ destructiveHint: true },
		async ({ device, x, y, mode, observe }) => {
			const robot = getRobot(device);
			if (mode === "double") {
				await robot.doubleTap(x, y);
			} else if (mode === "long") {
				await robot.longPress(x, y, 800);
			} else {
				await robot.tap(x, y);
			}

			return afterAction(device, observe !== false);
		}
	);

	tool(
		"type",
		"Type text into the focused input (tap it first). submit=true presses ENTER after.",
		{
			device,
			text: z.string(),
			submit: z.boolean().optional(),
			observe,
		},
		{ destructiveHint: true },
		async ({ device, text, submit, observe }) => {
			const robot = getRobot(device);
			await robot.sendKeys(text);
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
				await sleep(SETTLE_MS);
			} else {
				await robot.terminateApp(pkg);
			}

			return afterAction(device, observe !== false);
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
