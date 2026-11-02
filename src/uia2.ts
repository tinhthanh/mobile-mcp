import { spawn, ChildProcess } from "node:child_process";

import { trace } from "./logger";
import { AndroidRobot, getAdbPath } from "./android";

// appium uiautomator2 server, an instrumentation that keeps UiAutomation alive on the
// device. page source takes ~0.4s instead of ~2.7s for `uiautomator dump`, and
// setText types unicode (e.g. vietnamese) which `input text` cannot.
// install the official apks from https://github.com/appium/appium-uiautomator2-server/releases
const SERVER_PACKAGE = "io.appium.uiautomator2.server";
const TEST_PACKAGE = "io.appium.uiautomator2.server.test";
const RUNNER = `${TEST_PACKAGE}/androidx.test.runner.AndroidJUnitRunner`;
const DEVICE_PORT = 6790;
const START_TIMEOUT_MS = 20000;
const REQUEST_TIMEOUT_MS = 15000;

// while running, any app on the device can reach the server on localhost:6790,
// so stop it when idle instead of leaving it up
const IDLE_STOP_MS = 5 * 60 * 1000;

// some screens break page source for good (e.g. "Cannot set AccessibilityNodeInfo's field 'mSealed'")
const COOLDOWN_MS = 60 * 1000;

const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

const ELEMENT_KEY = "element-6066-11e4-a52e-4f735466cecf";

export class Uia2Client {

	private port = 0;
	private sessionId: string | null = null;
	private instrument: ChildProcess | null = null;
	private idleTimer: NodeJS.Timeout | null = null;
	private disabledUntil = 0;

	public constructor(private robot: AndroidRobot, private deviceId: string) {
	}

	public static isInstalled(robot: AndroidRobot): boolean {
		const packages = robot.adb("shell", "pm", "list", "packages", SERVER_PACKAGE).toString();
		return packages.includes(`package:${SERVER_PACKAGE}\n`) && packages.includes(`package:${TEST_PACKAGE}`);
	}

	private async request(method: string, path: string, body?: any): Promise<any> {
		const response = await fetch(`http://127.0.0.1:${this.port}${path}`, {
			method,
			headers: body ? { "Content-Type": "application/json" } : undefined,
			body: body ? JSON.stringify(body) : undefined,
			signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
		});

		const json: any = await response.json();
		if (!response.ok || json?.value?.error) {
			throw new Error(`uia2 ${method} ${path}: ${json?.value?.message || response.status}`);
		}

		return json.value;
	}

	private async isUp(): Promise<boolean> {
		try {
			const status = await this.request("GET", "/status");
			return status?.ready === true;
		} catch (error: any) {
			return false;
		}
	}

	private async start(): Promise<void> {
		this.port = Number(this.robot.adb("forward", "tcp:0", `tcp:${DEVICE_PORT}`).toString().trim());
		if (await this.isUp()) {
			return;
		}

		trace(`starting uiautomator2 server on ${this.deviceId}, local port ${this.port}`);
		this.robot.adb("shell", "am", "force-stop", SERVER_PACKAGE);
		this.instrument = spawn(getAdbPath(), ["-s", this.deviceId, "shell", "am", "instrument", "-w", "-e", "disableAnalytics", "true", RUNNER], {
			stdio: "ignore",
		});
		this.instrument.unref();

		const deadline = Date.now() + START_TIMEOUT_MS;
		while (Date.now() < deadline) {
			if (await this.isUp()) {
				return;
			}

			await sleep(300);
		}

		throw new Error("uiautomator2 server did not start");
	}

	private async session(): Promise<string> {
		if (this.sessionId) {
			return this.sessionId;
		}

		await this.start();
		const created = await this.request("POST", "/session", { capabilities: { alwaysMatch: { platformName: "Android" } } });
		const sessionId: string = created.sessionId ?? (await this.request("GET", "/sessions"))[0]?.id;
		// the default waits for an idle ui, which is what makes `uiautomator dump` slow
		await this.request("POST", `/session/${sessionId}/appium/settings`, { settings: { waitForIdleTimeout: 0, waitForSelectorTimeout: 0 } });
		this.sessionId = sessionId;
		return sessionId;
	}

	private touch() {
		if (this.idleTimer) {
			clearTimeout(this.idleTimer);
		}

		this.idleTimer = setTimeout(() => this.stop(), IDLE_STOP_MS);
		this.idleTimer.unref();
	}

	// runs fn against a live session, recreating it if the server went away
	private async withSession<T>(fn: (sessionId: string) => Promise<T>): Promise<T | null> {
		if (Date.now() < this.disabledUntil) {
			return null;
		}

		this.touch();
		for (let attempt = 0; attempt < 3; attempt++) {
			try {
				return await fn(await this.session());
			} catch (error: any) {
				// source fails now and then while the ui is mid-transition, so only
				// drop the session when the server itself is gone
				trace(`uia2 failed (attempt ${attempt + 1}): ${error.message}`);
				if (!this.port || !await this.isUp()) {
					this.sessionId = null;
				}

				await sleep(200);
			}
		}

		// fall back to adb for a while; uiautomator dump cannot run while the server holds UiAutomation
		this.disabledUntil = Date.now() + COOLDOWN_MS;
		this.stop();
		return null;
	}

	public async getSource(): Promise<string | null> {
		return this.withSession(sessionId => this.request("GET", `/session/${sessionId}/source`));
	}

	// replaces the focused input's text (setText ignores replace=false on uia2 7.x).
	// pasting through the clipboard would append, but it clobbers the user's clipboard
	// and android 13's "copied" overlay breaks page source (hidden api mSealed error).
	public async setFocusedText(text: string): Promise<boolean> {
		const done = await this.withSession(async sessionId => {
			const element = await this.request("POST", `/session/${sessionId}/element`, {
				strategy: "-android uiautomator",
				selector: "new UiSelector().focused(true)",
				context: "",
				multiple: false,
			});
			const elementId = element[ELEMENT_KEY] || element.ELEMENT;
			await this.request("POST", `/session/${sessionId}/element/${elementId}/value`, { text, replace: true });
			return true;
		});

		return done === true;
	}

	public stop() {
		if (this.idleTimer) {
			clearTimeout(this.idleTimer);
			this.idleTimer = null;
		}

		this.sessionId = null;
		try {
			this.robot.adb("shell", "am", "force-stop", SERVER_PACKAGE);
			if (this.port) {
				this.robot.adb("forward", "--remove", `tcp:${this.port}`);
			}
		} catch (error: any) {
			// device may be gone already
		}

		this.instrument?.kill();
		this.instrument = null;
		this.port = 0;
	}
}
