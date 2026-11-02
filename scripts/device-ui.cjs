#!/usr/bin/env node
// Small UI driver used by scripts/setup.sh (needs `npm run build` first).
//   device-ui.cjs <serial> screen
//   device-ui.cjs <serial> tap-text <text>     exit 1 when no element matches
//   device-ui.cjs <serial> tap-id <resource-id> exit 1 when absent (locale independent, e.g. android:id/button1)
//   device-ui.cjs <serial> token                prints the first 32+ char [A-Za-z0-9_-] text on screen
//   device-ui.cjs <serial> uia2-check           starts uiautomator2, reads the screen once, prints ms; exit 1 on failure
const { AndroidRobot } = require("../lib/android.js");
const { compactScreen, findLine } = require("../lib/screen.js");
const { Uia2Client } = require("../lib/uia2.js");

const [serial, cmd, ...rest] = process.argv.slice(2);
const arg = rest.join(" ");
const robot = new AndroidRobot(serial);
const sleep = ms => new Promise(r => setTimeout(r, ms));

const main = async () => {
	if (cmd === "uia2-check") {
		const client = new Uia2Client(robot, serial);
		const started = Date.now();
		try {
			const source = await client.getSource();
			if (!source || !source.includes("<")) {
				return 1;
			}

			process.stdout.write(String(Date.now() - started));
			return 0;
		} finally {
			client.stop();
		}
	}

	const dump = await robot.getUiAutomatorDump();
	if (cmd === "screen") {
		const { app, lines } = compactScreen(dump);
		console.log([`app=${app}`, ...lines].join("\n"));
		return 0;
	}

	if (cmd === "tap-text") {
		const { app, lines } = compactScreen(dump);
		const found = findLine([`app=${app}`, ...lines].join("\n"), arg);
		if (!found) {
			return 1;
		}

		await robot.tap(found.x, found.y);
		await sleep(1200);
		return 0;
	}

	if (cmd === "tap-id") {
		const node = dump.match(new RegExp(`<node [^>]*resource-id="${arg.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}"[^>]*>`));
		const bounds = node && node[0].match(/bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"/);
		if (!bounds) {
			return 1;
		}

		const [, l, t, r, b] = bounds.map(Number);
		await robot.tap((l + r) >> 1, (t + b) >> 1);
		await sleep(1200);
		return 0;
	}

	if (cmd === "token") {
		const token = dump.match(/text="([A-Za-z0-9_-]{32,})"/);
		if (!token) {
			return 1;
		}

		process.stdout.write(token[1]);
		return 0;
	}

	console.error(`unknown command ${cmd}`);
	return 2;
};

main().then(code => process.exit(code), err => {
	console.error(err.message);
	process.exit(1);
});
