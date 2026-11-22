#!/usr/bin/env bash
# Run one android-tester task headless with the leanest context: the agent's prompt as the whole
# system prompt, no built-in tools and only the berth-mobile MCP server (~3.5k tokens per turn
# instead of ~6.5k with `claude -p --agent android-tester`). Uses Shipyard's "mobile" plugin
# through berthd when it is installed, else berth-mobile built in this repository.
#
#   scripts/run-test.sh "Mở Cài đặt, vào Display: Dark mode đang On hay Off?"
#   MODEL=haiku scripts/run-test.sh "..."      # default sonnet
#   STREAM=run.jsonl scripts/run-test.sh "..."  # also keep the full stream-json transcript
#
# Prints the agent's report, then one stats line (turns, seconds, cost). Exit code 1 when the
# report does not start with PASS.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
AGENT="$REPO/.claude/agents/android-tester.md"
TASK="${1:?usage: scripts/run-test.sh \"<task>\"}"

command -v claude >/dev/null || { echo "claude CLI not found" >&2; exit 2; }
BERTHD="$(command -v berthd || echo "$HOME/.local/bin/berthd")"
LOCAL="$REPO/berth-mobile/target/release/berth-mobile"
if [ -x "$BERTHD" ] && [ -f "$HOME/.berth/plugins/mobile/berth-plugin.json" ]; then
	SERVER=$(printf '{"command":"%s","args":["plugin-mcp","mobile","mobile"]}' "$BERTHD")
elif [ -x "$LOCAL" ]; then
	SERVER=$(printf '{"command":"%s","args":["mcp"]}' "$LOCAL")
else
	echo "no berth-mobile: install Shipyard's mobile plugin, or run scripts/setup.sh" >&2
	exit 2
fi

# the agent body without its frontmatter
PROMPT=$(awk 'f > 1 { print } /^---$/ { f++ }' "$AGENT")

WORK=$(mktemp -d)
trap 'rm -rf "${WORK:?}"' EXIT
printf '{"mcpServers":{"berth-mobile":%s}}' "$SERVER" > "$WORK/mcp.json"

claude -p "$TASK" \
	--model "${MODEL:-sonnet}" \
	--system-prompt "$PROMPT" \
	--tools "" \
	--strict-mcp-config --mcp-config "$WORK/mcp.json" \
	--allowedTools "mcp__berth-mobile__*" \
	--output-format stream-json --verbose < /dev/null > "$WORK/run.jsonl"

[ -n "${STREAM:-}" ] && cp "$WORK/run.jsonl" "$STREAM"

python3 -I - "$WORK/run.jsonl" <<'EOF'
import json, sys

result = [json.loads(line) for line in open(sys.argv[1]) if line.strip()][-1]
report = result.get("result", "")
print(report)
print(f"\n[{result.get('num_turns')} turns, {result.get('duration_ms', 0) / 1000:.0f}s, ${result.get('total_cost_usd', 0):.4f}]")
# the agent's report opens with "1. Result: PASS / FAIL / BLOCKED"
head = report.lstrip("*# 1.").upper()[:60]
sys.exit(0 if "PASS" in head and "FAIL" not in head else 1)
EOF
