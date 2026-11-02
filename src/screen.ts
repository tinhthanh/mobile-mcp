import * as xml from "fast-xml-parser";

interface UiNode {
	node?: UiNode | UiNode[];
	class?: string;
	package?: string;
	text?: string;
	"content-desc"?: string;
	"resource-id"?: string;
	hint?: string;
	bounds?: string;
	clickable?: string;
	"long-clickable"?: string;
	scrollable?: string;
	checkable?: string;
	checked?: string;
	selected?: string;
	enabled?: string;
	focused?: string;
	// appium page source only
	displayed?: string;
}

interface Rect {
	left: number;
	top: number;
	right: number;
	bottom: number;
}

// one line per element: "<mark><label>|<x>,<y>[ flags]", x,y is the tap point (center)
// marks: "*" tappable, "#" text input, "~" scrollable, " " plain text
export const SCREEN_LEGEND = "Lines are \"<mark><label>|x,y [flags]\" where x,y is the center to tap. " +
	"Marks: * tappable, # text input, ~ scrollable, space = plain text. Flags: on (checked), sel (selected), off (disabled), focus.";

const MAX_LABEL = 60;

// a "tappable" view covering this much of the screen is a container (e.g. a clickable root
// FrameLayout in zalo); merging its texts would collapse the whole screen into one line
const CONTAINER_AREA = 0.4;

const children = (node: UiNode): UiNode[] => {
	if (!node.node) {
		return [];
	}

	return Array.isArray(node.node) ? node.node : [node.node];
};

const parseBounds = (bounds?: string): Rect | null => {
	const m = String(bounds).match(/^\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\]$/);
	if (!m) {
		return null;
	}

	const [, left, top, right, bottom] = m.map(Number);
	return { left, top, right, bottom };
};

// the parser leaves numeric references such as "&#10;" (newline) in attribute values
const decodeCharRefs = (s: string): string => s.replace(/&#(x[0-9a-fA-F]+|\d+);/g, (_, code: string) =>
	String.fromCodePoint(code[0] === "x" ? parseInt(code.substring(1), 16) : parseInt(code, 10)));

const ownLabel = (node: UiNode): string => {
	const s = (v?: string) => decodeCharRefs(v === undefined ? "" : String(v)).replace(/\s+/g, " ").trim();
	return s(node.text) || s(node["content-desc"]) || s(node.hint);
};

const shortId = (node: UiNode): string => {
	const id = node["resource-id"] ? String(node["resource-id"]) : "";
	return id.substring(id.lastIndexOf("/") + 1);
};

const shortClass = (node: UiNode): string => {
	const cls = node.class || "View";
	return cls.substring(cls.lastIndexOf(".") + 1);
};

const isTrue = (v?: string) => String(v) === "true";

const isInput = (node: UiNode) => /EditText|AutoCompleteTextView/.test(node.class || "");

const isTappable = (node: UiNode) => isTrue(node.clickable) || isTrue(node["long-clickable"]) || isTrue(node.checkable);

// texts of non-interactive descendants, which belong to the tappable ancestor
const collectTexts = (node: UiNode, out: string[]) => {
	for (const child of children(node)) {
		if (isTappable(child) || isInput(child)) {
			continue;
		}

		const label = ownLabel(child);
		if (label && !out.includes(label)) {
			out.push(label);
		}

		collectTexts(child, out);
	}
};

const truncate = (s: string) => (s.length > MAX_LABEL ? s.substring(0, MAX_LABEL - 1) + "…" : s);

export interface CompactScreen {
	app: string;
	lines: string[];
}

// appium's page source names each element after its class (<android.widget.TextView ...>)
// where uiautomator dump uses <node ...>; the attributes are the same
const normalizeTags = (dump: string): string => dump.replace(/<(\/?)(?!hierarchy[\s>])([A-Za-z_][\w.$]*)(?=[\s/>])/g, "<$1node");

export const compactScreen = (dump: string): CompactScreen => {
	const parser = new xml.XMLParser({ ignoreAttributes: false, attributeNamePrefix: "", parseAttributeValue: false });
	const parsed = parser.parse(normalizeTags(dump.substring(dump.indexOf("<?xml"))));
	const roots = children(parsed.hierarchy || {});
	const screen = roots.map(r => parseBounds(r.bounds)).find(r => r !== null) || null;

	interface Entry {
		line: string;
		point: string;
		// label is a resource id or class name, not something a person reads
		fallback: boolean;
	}

	const entries: Entry[] = [];
	const visible = (r: Rect) => {
		if (r.right <= r.left || r.bottom <= r.top) {
			return false;
		}

		if (!screen) {
			return true;
		}

		const cx = (r.left + r.right) / 2, cy = (r.top + r.bottom) / 2;
		return cx >= screen.left && cx <= screen.right && cy >= screen.top && cy <= screen.bottom;
	};

	const emit = (mark: string, label: string, fallback: boolean, r: Rect, node: UiNode) => {
		const flags: string[] = [];
		if (isTrue(node.checked)) {
			flags.push("on");
		}

		if (isTrue(node.selected)) {
			flags.push("sel");
		}

		if (node.enabled !== undefined && !isTrue(node.enabled)) {
			flags.push("off");
		}

		if (isInput(node) && isTrue(node.focused)) {
			flags.push("focus");
		}

		const point = `${(r.left + r.right) >> 1},${(r.top + r.bottom) >> 1}`;
		const line = `${mark}${truncate(label)}|${point}` + (flags.length ? ` ${flags.join(" ")}` : "");
		if (!entries.some(e => e.line === line)) {
			entries.push({ line, point, fallback });
		}
	};

	const screenArea = screen ? (screen.right - screen.left) * (screen.bottom - screen.top) : 0;
	const actionable = (node: UiNode): boolean => {
		if (isInput(node)) {
			return true;
		}

		if (!isTappable(node)) {
			return false;
		}

		const r = parseBounds(node.bounds);
		return !r || !screenArea || (r.right - r.left) * (r.bottom - r.top) < screenArea * CONTAINER_AREA;
	};

	const walk = (node: UiNode) => {
		const r = parseBounds(node.bounds);
		if (r && visible(r) && node.displayed !== "false") {
			if (actionable(node)) {
				const texts: string[] = [];
				const own = ownLabel(node);
				if (own) {
					texts.push(own);
				}

				collectTexts(node, texts);
				const label = texts.join(" · ");
				emit(isInput(node) ? "#" : "*", label || shortId(node) || shortClass(node), !label, r, node);
			} else if (isTrue(node.scrollable)) {
				emit("~", ownLabel(node) || shortId(node) || shortClass(node), !ownLabel(node), r, node);
			} else if (ownLabel(node) && !hasTappableAncestor.has(node)) {
				emit(" ", ownLabel(node), false, r, node);
			}
		}

		for (const child of children(node)) {
			if (actionable(node) || hasTappableAncestor.has(node)) {
				hasTappableAncestor.add(child);
			}

			walk(child);
		}
	};

	const hasTappableAncestor = new Set<UiNode>();
	roots.forEach(walk);

	// widgets often stack an id-only view on top of the labelled one ("search_widget_ghost_lens" over
	// "Camera search"); keep the readable line for that tap point
	const labelled = new Set(entries.filter(e => !e.fallback).map(e => e.point));
	const seen = new Set<string>();
	const lines = entries.filter(e => {
		if (!e.fallback) {
			return true;
		}

		// several unlabelled views on one point ("~RecyclerView", "~contentList") need only one line
		const keep = !labelled.has(e.point) && !seen.has(e.point);
		seen.add(e.point);
		return keep;
	}).map(e => e.line);

	return { app: roots.find(r => r.package)?.package || "unknown", lines };
};

const normalize = (s: string) => s.normalize("NFC").toLowerCase().replace(/\s+/g, " ").trim();

// finds the line to tap for text: exact label part on a tappable line wins,
// then exact on any line, then substring
export const findLine = (screen: string, text: string): { line: string, x: number, y: number, matches: number } | null => {
	const wanted = normalize(text);
	const rows = screen.split("\n").slice(1).map(line => {
		const bar = line.lastIndexOf("|");
		const [x, y] = line.substring(bar + 1).split(" ")[0].split(",").map(Number);
		const parts = line.substring(1, bar).split(" · ").map(normalize);
		const tappable = line[0] === "*" || line[0] === "#";
		const exact = parts.includes(wanted);
		const partial = parts.some(p => p.includes(wanted));
		const rank = exact && tappable ? 0 : exact ? 1 : partial && tappable ? 2 : partial ? 3 : 9;
		return { line, x, y, rank };
	}).filter(row => row.rank < 9);

	if (rows.length === 0) {
		return null;
	}

	const best = Math.min(...rows.map(row => row.rank));
	const top = rows.filter(row => row.rank === best);
	return { ...top[0], matches: top.length };
};
