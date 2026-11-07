---
title: Installation
description: Install agents-tmux via TPM or manually.
---

## Requirements

- tmux 3.0+
- [TPM](https://github.com/tmux-plugins/tpm) (for plugin installation)
- [GitHub CLI](https://cli.github.com/) (optional, for displaying PR numbers in the Git tab)
- [Rust](https://rustup.rs/) (only if building from source)

## Option A — TPM (recommended)

Add the plugin to your `tmux.conf`:

```bash
set -g @plugin 'ahmad9059/AgentsTmux'
```

Reload `tmux.conf`, then press `prefix + I` to install:

```sh
tmux source ~/.tmux.conf
```

On the first run, an install wizard prompts you to download a pre-built binary or build from source.

To update later, press `prefix + U` in TPM's plugin list and select `agents-tmux`. The install wizard runs again if the bundled binary has changed.

## Option B — Manual

1. Clone the repository:

   ```sh
   git clone https://github.com/ahmad9059/AgentsTmux.git \
     ~/.tmux/plugins/AgentsTmux
   ```

2. Add the plugin to your `tmux.conf`:

   ```bash
   run-shell ~/.tmux/plugins/AgentsTmux/agents-tmux.tmux
   ```

3. Install the binary — download a pre-built release, or build from source:

   ```sh
   # macOS (Apple Silicon)
   curl -fSL https://github.com/ahmad9059/AgentsTmux/releases/latest/download/agents-tmux-darwin-aarch64 \
     -o ~/.tmux/plugins/AgentsTmux/bin/agents-tmux
   chmod +x ~/.tmux/plugins/AgentsTmux/bin/agents-tmux
   ```

   Or build from source:

   ```sh
   cd ~/.tmux/plugins/AgentsTmux
   cargo build --release
   ```

## Reload tmux config

After editing `tmux.conf`, press `prefix + r` (or run `tmux source ~/.tmux.conf`) to reload.

## Next steps

The sidebar receives status updates through agent hooks — continue with the agent you use:

- [Claude Code setup](/AgentsTmux/getting-started/claude-code/)
- [Codex setup](/AgentsTmux/getting-started/codex/)
- [OpenCode setup](/AgentsTmux/getting-started/opencode/)
