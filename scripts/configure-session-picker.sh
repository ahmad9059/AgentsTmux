#!/usr/bin/env bash
# Filter internal holding sessions from the standard tmux pickers only.
# Custom bindings and existing filters belong to the user and stay untouched.
set -eu
if [[ "$(tmux show-option -gqv @agentstmux_filter_session_picker)" == "off" ]]; then
    exit 0
fi
for key in s w; do
    binding="$(tmux list-keys -T prefix "$key" 2>/dev/null || true)"
    case "$binding" in
        *" $key choose-tree -Z$key"|*" $key choose-tree -$key")
            tmux bind-key -T prefix "$key" choose-tree "-Z$key" \
                -f '#{==:#{m:__agent-sidebar-*,#{session_name}},0}'
            ;;
    esac
done
