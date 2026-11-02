#!/usr/bin/env bash
# Run one android-tester task headless with the leanest context: the agent's prompt as the whole
# system prompt, no built-in tools and only the mobile-lite MCP server (~3.5k tokens per turn
# instead of ~6.5k with `claude -p --agent android-tester`).
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
[ -f "$REPO/lib/index.js" ] || { echo "run scripts/setup.sh host first" >&2; exit 2; }

# the agent body without its frontmatter
PROMPT=$(awk 'f > 1 { print } /^---$/ { f++ }' "$AGENT")

WORK=$(mktemp -d)
trap 'rm -rf "${WORK:?}"' EXIT
printf '{"mcpServers":{"mobile-lite":{"command":"node","args":["%s/lib/index.js"]}}}' "$REPO" > "$WORK/mcp.json"

claude -p "$TASK" \
	--model "${MODEL:-sonnet}" \
	--system-prompt "$PROMPT" \
	--tools "" \
	--strict-mcp-config --mcp-config "$WORK/mcp.json" \
	--allowedTools "mcp__mobile-lite__*" \
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
