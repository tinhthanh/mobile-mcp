import fs from "node:fs";
import path from "node:path";
import { test, expect } from "@playwright/test";

import { compactScreen, findLine } from "../src/screen";

const fixture = (name: string) => fs.readFileSync(path.join(__dirname, "fixtures", name), "utf8");

const node = (attrs: string, inner = "") => `<node ${attrs}>${inner}</node>`;
const dump = (inner: string) => `junk<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation="0">${node(`package="com.example" bounds="[0,0][720,1600]"`, inner)}</hierarchy>`;

test.describe("compact screen", () => {

	test("merges child texts into the tappable row and drops layout noise", async () => {
		const { app, lines } = compactScreen(fixture("settings.xml"));
		expect(app).toBe("com.android.settings");
		expect(lines).toContain("*Wi‑Fi · Example WiFi|360,375");
		expect(lines).toContain("*Bluetooth · Off|360,495");
		expect(lines).toContain("*Display|360,1159");
		expect(lines).toContain("#Search settings|371,267");
		expect(lines.some(l => l.includes("dashboard_tile"))).toBe(false);
		expect(lines.some(l => l.startsWith(" Display"))).toBe(false);
		// the raw dump is ~9k tokens, mobile-mcp's element list ~2.3k
		expect(lines.join("\n").length / 4).toBeLessThan(150);
	});

	test("keeps nested tappables separate from their parent", async () => {
		const subtitle = node(`text="subtitle" bounds="[0,50][300,100]"`);
		const toggle = node(`clickable="true" checkable="true" checked="true" resource-id="a:id/switch" bounds="[600,0][720,100]"`);
		const xml = dump(node(`clickable="true" text="Row" bounds="[0,0][720,100]"`, subtitle + toggle));
		expect(compactScreen(xml).lines).toEqual([
			"*Row · subtitle|360,50",
			"*switch|660,50 on",
		]);
	});

	test("reads appium page source, where tags are class names", async () => {
		const xml = `<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy index="0" class="hierarchy" rotation="0" width="720" height="1522">` +
			`<android.widget.FrameLayout package="com.example" bounds="[0,0][720,1600]" displayed="true">` +
			`<android.widget.LinearLayout clickable="true" bounds="[0,0][720,100]" displayed="true">` +
			`<android.widget.TextView text="Hiển thị" bounds="[0,0][300,100]" displayed="true"/>` +
			`</android.widget.LinearLayout>` +
			`<android.widget.TextView text="hidden" bounds="[0,100][300,200]" displayed="false"/>` +
			`</android.widget.FrameLayout></hierarchy>`;
		expect(compactScreen(xml)).toEqual({ app: "com.example", lines: ["*Hiển thị|360,50"] });
	});

	test("skips zero-size and offscreen nodes", async () => {
		const xml = dump(
			node(`text="gone" bounds="[0,0][0,0]"`) +
			node(`text="below" bounds="[0,1700][720,1800]"`) +
			node(`text="here" bounds="[0,0][100,100]"`));
		expect(compactScreen(xml).lines).toEqual([" here|50,50"]);
	});

	test("flags disabled tappables and focused inputs", async () => {
		const xml = dump(
			node(`clickable="true" enabled="false" text="Next" bounds="[0,0][100,100]"`) +
			node(`class="android.widget.EditText" focused="true" text="" hint="Email" bounds="[0,100][720,200]"`));
		expect(compactScreen(xml).lines).toEqual([
			"*Next|50,50 off",
			"#Email|360,150 focus",
		]);
	});

	test("does not merge texts into a tappable view covering most of the screen", async () => {
		const texts = node(`text="Messages" bounds="[0,300][200,350]"`) + node(`text="Links&#10;Files" bounds="[0,400][720,450]"`);
		const xml = dump(node(`clickable="true" bounds="[0,0][720,1500]"`, texts));
		expect(compactScreen(xml).lines).toEqual([" Messages|100,325", " Links Files|360,425"]);
	});

	test("drops an id-only line stacked on a labelled one at the same point", async () => {
		const xml = dump(
			node(`clickable="true" resource-id="g:id/search_widget_ghost_lens" bounds="[600,400][660,500]"`) +
			node(`clickable="true" content-desc="Camera search" bounds="[600,400][660,500]"`) +
			node(`clickable="true" resource-id="g:id/only_id" bounds="[0,400][100,500]"`));
		expect(compactScreen(xml).lines).toEqual(["*Camera search|630,450", "*only_id|50,450"]);
	});
});

test.describe("find line", () => {

	const screen = [
		"app=com.android.settings",
		" Display|412,159",
		"*Display size · Default|360,1170",
		"*Display|360,1159",
		"*Hiển thị|360,200",
	].join("\n");

	test("prefers an exact label on a tappable line", async () => {
		expect(findLine(screen, "display")).toMatchObject({ x: 360, y: 1159, matches: 1 });
	});

	test("matches a merged label part and unicode text", async () => {
		expect(findLine(screen, "Default")).toMatchObject({ x: 360, y: 1170 });
		expect(findLine(screen, "hiển thị")).toMatchObject({ x: 360, y: 200 });
	});

	test("treats typographic quotes and dashes like plain ones", async () => {
		const dialog = "app=com.google.android.permissioncontroller\n*Don’t allow|360,1036\n*Wi‑Fi – settings|360,400";
		expect(findLine(dialog, "Don't allow")).toMatchObject({ x: 360, y: 1036 });
		expect(findLine(dialog, "wi-fi - settings")).toMatchObject({ y: 400 });
	});

	test("returns null when nothing matches", async () => {
		expect(findLine(screen, "Bluetooth")).toBeNull();
	});
});
