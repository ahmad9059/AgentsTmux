---
title: OpenCode setup
description: Install the AgentsTmux client-side bridge for OpenCode V2.
---

## OpenCode V2

The plugin configures OpenCode V2 automatically when it loads in tmux and `~/.config/opencode` exists. To run it by hand:

```sh
agents-tmux integrate --opencode
```

This preserves existing CLI settings and adds the package in `integrations/opencode-v2` to `~/.config/opencode/cli.json`. Use `/reload` if the client has not loaded the plugin yet.

The bridge runs in the TUI, not the shared server, so `TMUX_PANE` identifies the correct terminal. New questions, permission prompts, errors, and completions update the sidebar. Switching sessions silently synchronizes state instead of replaying old notifications.

Do not load the V1 bridge in OpenCode V2. If migrating from a previous setup, remove its old sidebar plugin entry after enabling AgentsTmux so two bridges do not emit duplicate events.

## Legacy OpenCode V1

The legacy bridge is retained at `integrations/opencode-v1/agents-tmux.js`. It uses the V1 plugin API and is separate from the supported V2 TUI package.
