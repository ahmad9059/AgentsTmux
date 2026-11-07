#!/usr/bin/env bash
# Capture the real sidebar with isolated, synthetic agent and usage data.
# No real credentials, conversations, activity logs, or tmux sessions are used.
# Usage: bash fixtures/scenarios/readme/scenario.sh <output-directory>
# Render: (cd website && node ../scripts/render-frames.mjs <output-directory> --scale 2 --readme)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
OUT="$(mkdir -p "${1:?output directory required}" && cd "$1" && pwd)"
BIN="$ROOT/target/release/agents-tmux"
SOURCE="$ROOT/fixtures/scenarios/readme"
[[ -x "$BIN" ]] || { echo 'Run cargo build --release first.' >&2; exit 1; }

# Reserve pane IDs above the user's existing panes. Activity log names use
# pane IDs globally, even across tmux servers. Never read or delete those logs.
LAST_PANE=$(tmux list-panes -a -F '#{pane_id}' 2>/dev/null | tr -d '%' | sort -n | tail -1 || true)
MIN_ID=$(( ${LAST_PANE:-0} + 100 ))
unset TMUX TMUX_PANE
WORK="$(mktemp -d "${TMPDIR:-/tmp}/agentstmux-readme.XXXXXX")"
SOCKET="$WORK/tmux.sock"
tmux() { command tmux -S "$SOCKET" "$@"; }
cleanup() {
    # The explicit socket prevents cleanup from reaching the user's server.
    tmux kill-server 2>/dev/null || true
    if [[ "$WORK" == */agentstmux-readme.* ]]; then rm -rf "$WORK"; fi
}
trap cleanup EXIT

export HOME="$WORK/home" XDG_CACHE_HOME="$WORK/home/.cache" XDG_CONFIG_HOME="$WORK/home/.config"
mkdir -p "$HOME/.claude" "$HOME/.codex" "$WORK/bin"
export PATH="$WORK/bin:$PATH"
export AGENTSTMUX_SHOW_PREVIEWS=0
for agent in claude codex opencode; do ln -s /bin/sleep "$WORK/bin/$agent"; done

# This curl replacement supplies documented demo values without network
# requests. The actual UsageFetcher and native journal parsers still run.
cat > "$WORK/bin/curl" <<'CURL'
#!/usr/bin/env python3
import json, sys
from pathlib import Path
url = sys.argv[-1]
if url.endswith('/api/oauth/usage'):
    payload = json.loads((Path.home() / 'claude-demo-usage.json').read_text())
elif url.endswith('/backend-api/wham/usage'):
    payload = json.loads((Path.home() / 'codex-demo-usage.json').read_text())
else:
    payload = {}
if '--dump-header' in sys.argv:
    sys.stdout.write('HTTP/2 200\r\ncontent-type: application/json\r\n\r\n')
sys.stdout.write(json.dumps(payload))
CURL
chmod +x "$WORK/bin/curl"

python3 - <<'PY'
import datetime, json, os, pathlib, time
h = pathlib.Path.home()
now = int(time.time())
iso = lambda seconds: datetime.datetime.fromtimestamp(seconds, datetime.timezone.utc).isoformat()
def save(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data))
save(h/'.claude/.credentials.json', {'claudeAiOauth': {'accessToken': 'readme-demo-not-a-credential', 'subscriptionType': 'max'}})
save(h/'.codex/auth.json', {'tokens': {'access_token': 'readme-demo-not-a-credential', 'account_id': 'demo'}})
save(h/'claude-demo-usage.json', {
    'five_hour': {'utilization': 38, 'resets_at': iso(now+2*3600+20*60)},
    'seven_day': {'utilization': 64, 'resets_at': iso(now+3*86400+4*3600)},
    'extra_usage': {'is_enabled': True, 'used_credits': 425, 'monthly_limit': 2000}})
save(h/'codex-demo-usage.json', {'plan_type': 'pro', 'rate_limit': {
    'primary_window': {'used_percent': 24, 'limit_window_seconds': 18000, 'reset_at': now+4*3600+10*60},
    'secondary_window': {'used_percent': 57, 'limit_window_seconds': 604800, 'reset_at': now+4*86400+2*3600}}})
rows = [
    {'type': 'assistant', 'timestamp': iso(now), 'message': {'id': 'demo-response', 'model': 'claude-opus-5-5',
        'usage': {'input_tokens': 2200, 'output_tokens': 800, 'cache_read_input_tokens': 14000}}},
    {'type': 'cost-state', 'totalCostUSD': .42, 'modelUsage': {'claude-opus-5-5': {
        'inputTokens': 2200, 'outputTokens': 800, 'cacheReadInputTokens': 14000}}}]
p = h/'.claude/projects/demo/demo-claude.jsonl'
p.parent.mkdir(parents=True, exist_ok=True)
p.write_text(''.join(json.dumps(row)+'\n' for row in rows))
rows = [
    {'type': 'turn_context', 'payload': {'model': 'gpt-6.1-sol'}},
    {'timestamp': iso(now), 'payload': {'type': 'token_count', 'info': {'total_token_usage': {'total_tokens': 28400}}}}]
p = h/'.codex/sessions/2026/11/07/rollout-demo-codex.jsonl'
p.parent.mkdir(parents=True, exist_ok=True)
p.write_text(''.join(json.dumps(row)+'\n' for row in rows))
PY

tmux -f /dev/null new-session -d -s readme -x 124 -y 38 'exec /bin/sleep 999999'
tmux set-option -g status off
tmux set-option -g default-shell /bin/bash
tmux set-option -g pane-border-style 'fg=#414868'
tmux set-option -g pane-active-border-style 'fg=#7aa2f7'
tmux set-option -g @sidebar_width 38
tmux set-option -g @sidebar_single_window on
tmux set-option -g @sidebar_bottom_height 20
tmux set-environment -g AGENTSTMUX_SHOW_PREVIEWS 0

new_pane() {
    local pane id
    while true; do
        pane=$(tmux new-window -d -t readme -P -F '#{pane_id}' -c "$HOME/projects/api" 'exec /bin/sleep 999999')
        id="${pane#%}"
        if (( id >= MIN_ID )) && [[ ! -e "/tmp/agentstmux-activity_${id}.log" ]]; then
            echo "$pane"
            return
        fi
        tmux kill-pane -t "$pane"
    done
}

for project in api web docs; do
    mkdir -p "$HOME/projects/$project"
    git -C "$HOME/projects/$project" init -q -b main
    printf '# Demo project\n' > "$HOME/projects/$project/README.md"
    git -C "$HOME/projects/$project" add README.md
    git -C "$HOME/projects/$project" -c user.name=Demo -c user.email=demo@example.invalid commit -qm 'Demo workspace'
done

SIDEBAR=$(new_pane)
MAIN=$(new_pane)
CODEX=$(new_pane)
OTHER=$(new_pane)
ERROR=$(new_pane)
WINDOW=$(tmux display-message -p -t "$SIDEBAR" '#{window_id}')
tmux rename-window -t "$WINDOW" Agents
tmux join-pane -h -t "$SIDEBAR" -s "$MAIN"
tmux resize-pane -t "$SIDEBAR" -x 38

seed() {
    local pane="$1" agent="$2" status="$3" project="$4" session="$5"
    tmux set-option -p -t "$pane" @pane_agent "$agent"
    tmux set-option -p -t "$pane" @pane_status "$status"
    tmux set-option -p -t "$pane" @pane_cwd "$HOME/projects/$project"
    tmux set-option -p -t "$pane" @pane_session_id "$session"
}
tmux set-option -p -t "$SIDEBAR" @pane_role sidebar
seed "$MAIN" claude running api demo-claude
seed "$CODEX" codex waiting web demo-codex
seed "$OTHER" opencode running docs demo-opencode
seed "$ERROR" claude error api demo-error
tmux set-option -p -t "$CODEX" @pane_attention notification
tmux set-option -p -t "$CODEX" @pane_wait_reason permission_required
tmux set-option -p -t "$ERROR" @pane_wait_reason 'Test command failed'
tmux respawn-pane -k -t "$MAIN" "cat '$SOURCE/claude.stream'; exec '$WORK/bin/claude' 999999"
tmux respawn-pane -k -t "$CODEX" "cat '$SOURCE/codex.stream'; exec '$WORK/bin/codex' 999999"
tmux respawn-pane -k -t "$OTHER" "$WORK/bin/opencode 999999"
tmux respawn-pane -k -t "$ERROR" "$WORK/bin/claude 999999"
tmux select-window -t "$WINDOW"
tmux select-pane -t "$MAIN"
tmux respawn-pane -k -t "$SIDEBAR" "$BIN"
export TMUX="$SOCKET,$(tmux display-message -p '#{pid}'),0"

# Wait for the real worker to load the fixture response before capture.
wait_for_usage() {
    local label="$1"
    for (( attempt=0; attempt<60; attempt++ )); do
        if tmux capture-pane -p -t "$SIDEBAR" | grep -q "$label"; then return; fi
        sleep 0.2
    done
    echo "Usage did not render: $label" >&2
    return 1
}
wait_for_usage 'Extra'
"$BIN" capture --session readme --window "$WINDOW" --frame-out "$OUT/overview.html"
tmux swap-pane -d -s "$CODEX" -t "$MAIN"
tmux select-pane -t "$CODEX"
wait_for_usage '28k tok'
"$BIN" capture --session readme --window "$WINDOW" --frame-out "$OUT/codex-usage.html"
