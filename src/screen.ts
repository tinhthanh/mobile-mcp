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

const ownLabel = (node: UiNode): string => {
	const s = (v?: string) => (v === undefined ? "" : String(v)).replace(/\s+/g, " ").trim();
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

export const compactScreen = (dump: string): CompactScreen => {
	const parser = new xml.XMLParser({ ignoreAttributes: false, attributeNamePrefix: "", parseAttributeValue: false });
	const parsed = parser.parse(dump.substring(dump.indexOf("<?xml")));
	const roots = children(parsed.hierarchy || {});
	const screen = roots.map(r => parseBounds(r.bounds)).find(r => r !== null) || null;

	const lines: string[] = [];
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

	const emit = (mark: string, label: string, r: Rect, node: UiNode) => {
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

		const x = (r.left + r.right) >> 1, y = (r.top + r.bottom) >> 1;
		const line = `${mark}${truncate(label)}|${x},${y}` + (flags.length ? ` ${flags.join(" ")}` : "");
		if (!lines.includes(line)) {
			lines.push(line);
		}
	};

	const walk = (node: UiNode) => {
		const r = parseBounds(node.bounds);
		if (r && visible(r)) {
			if (isTappable(node) || isInput(node)) {
				const texts: string[] = [];
				const own = ownLabel(node);
				if (own) {
					texts.push(own);
				}

				collectTexts(node, texts);
				const label = texts.join(" · ") || shortId(node) || shortClass(node);
				emit(isInput(node) ? "#" : "*", label, r, node);
			} else if (isTrue(node.scrollable)) {
				emit("~", ownLabel(node) || shortId(node) || shortClass(node), r, node);
			} else if (ownLabel(node) && !hasTappableAncestor.has(node)) {
				emit(" ", ownLabel(node), r, node);
			}
		}

		for (const child of children(node)) {
			if (isTappable(node) || isInput(node) || hasTappableAncestor.has(node)) {
				hasTappableAncestor.add(child);
			}

			walk(child);
		}
	};

	const hasTappableAncestor = new Set<UiNode>();
	roots.forEach(walk);

	return { app: roots.find(r => r.package)?.package || "unknown", lines };
};
