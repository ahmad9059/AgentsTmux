# AgentsTmux

One tmux window for your coding agents. Keep a persistent sidebar on the left, switch live agents on the right, and leave the rest running in the background.

## Features

- **One Agents tab:** new agents use a detached holding session instead of adding tabs to your main session. Selecting an agent swaps its live pane into view; its process, conversation, scrollback, and worktree stay intact.
- **Stable ordering:** agents stay in creation order within each project. New agents appear at the bottom instead of jumping around during swaps.
- **Reliable selection:** the visible agent stays highlighted while you type. Keyboard navigation has its own cursor, tracked by pane identity rather than an unstable row number.
- **Compact sidebar:** hide prompt/reply previews and the idle placeholder; retain status, branch, task progress, and activity.
- **Useful notifications:** short, single-line completion, question, permission, and error alerts. Session selection does not replay old completions. On Hyprland, alerts are suppressed when the source tmux window is visible on an active workspace.
- **Papirus status icons:** green completion tick, question, and error icons, with theme-icon fallbacks when Papirus is unavailable.
- **Claude Code, Codex, OpenCode V2, and Pi:** agent integrations are included, including the client-side bridge needed by OpenCode's shared-service architecture.
- **Worktree lifecycle:** create isolated agent worktrees and remove an individual agent without killing the shared Agents window.

## Requirements

- tmux 3.0+ and Git
- Rust/Cargo to build from source until AgentsTmux release binaries are published
- [TPM](https://github.com/tmux-plugins/tpm) for plugin-manager installation
- Python 3 for the optional integration installer
- Linux desktop notifications: `notify-send` and a notification daemon such as SwayNC
- Optional: Papirus icons, `hyprctl` for workspace-aware alerts, and `gh` for PR metadata

## Install with TPM

Add this before TPM's initialization line in `~/.tmux.conf`:

```tmux
set -g @plugin 'ahmad9059/AgentsTmux'
```

Reload tmux and press your prefix followed by **I**. In the install wizard, choose **Build from source** if a release binary is not yet available.

Enable the compact, single-window configuration:

```tmux
source-file ~/.tmux/plugins/AgentsTmux/examples/agents-tmux.conf
```

Reload your configuration with `tmux source-file ~/.tmux.conf`.

### Manual installation

```sh
git clone https://github.com/ahmad9059/AgentsTmux.git ~/.tmux/plugins/AgentsTmux
cd ~/.tmux/plugins/AgentsTmux
cargo build --release
```

Then add the example configuration and launcher to `~/.tmux.conf`:

```tmux
source-file ~/.tmux/plugins/AgentsTmux/examples/agents-tmux.conf
run-shell ~/.tmux/plugins/AgentsTmux/agents-tmux.tmux
```

## Agent setup

The installer backs up changed JSON files and preserves unrelated settings. Run it only for the agents you use:

```sh
python3 ~/.tmux/plugins/AgentsTmux/scripts/setup-integrations.py --all
# Or: --claude --codex --opencode --pi, individually or together
```

- **Claude Code:** installs `agents-tmux@agentstmux` from this repository's local marketplace. Start a new Claude session to load the plugin.
- **Codex:** merges lifecycle hooks into `~/.codex/hooks.json`. Review and trust the new definitions with `/hooks`; the installer does not bypass hook trust.
- **OpenCode V2:** adds the included TUI plugin to `~/.config/opencode/cli.json`. It runs in the local client so events target the correct `TMUX_PANE`, rather than the shared server's environment. Use `/reload` if needed.
- **Pi:** installs the included personal extension. Pi uses the Claude-compatible adapter and badge; its preview payloads are marked `[Pi]`. Native Pi branding in the Rust agent enum is not implemented.

The original V1-style OpenCode bridge is retained separately at `integrations/opencode-v1/agents-tmux.js`; do not load it in OpenCode V2.

When replacing an older sidebar installation, disable its plugin/hooks after enabling AgentsTmux. The installer intentionally does not remove differently named integrations from your settings; leaving both enabled can produce duplicate events.

For custom plugin locations, set `AGENTSTMUX_HOOK` to the absolute path of `hook.sh` before launching OpenCode or Pi.

## Usage

- **Prefix + e:** toggle the sidebar in the current window.
- **Prefix + E:** toggle sidebars across windows.
- **Click an agent**, or navigate with **j/k** or the arrow keys and press **Enter**, to show it in the right pane.
- Use the project **+** button to create a worktree and agent.
- Use an agent's **×** button for removal options. Removing an agent does not close the shared host window.

In single-window mode, other agents remain in a detached `__agent-sidebar-*` holding session. These are live panes, not suspended processes. Do not kill the holding session unless you intend to stop those agents.

### Configuration

See [`examples/agents-tmux.conf`](examples/agents-tmux.conf) for the complete configuration used by this setup. Key settings:

```tmux
set -g @sidebar_single_window on
set -g @sidebar_icon_running '⟳'
set -g @sidebar_icon_idle '✓'
set -g @sidebar_color_idle '#4caf50'
set-environment -g AGENTSTMUX_SHOW_PREVIEWS 0
```

The first spawn from a sidebar records that window as its session's host. Existing agents in other windows are not silently migrated. Advanced users can adopt a pane with `agents-tmux workspace-select <host-window> <pane> <owner-session>`; existing worktree markers are promoted to pane scope before swapping. Moving an existing window out of the main tab bar is a separate, explicit tmux operation.

Restart only the sidebar (toggle off, then on) after installing a newly built binary or changing startup-only appearance settings. Your agents can keep running.

## Development

```sh
cargo fmt
cargo test
cargo clippy
cargo build --release
```

Additional tests cover stable ordering, persistent highlighting, silent session snapshots, and question notifications. [`tests/workspace_tests.rs`](tests/workspace_tests.rs) contains an opt-in real-tmux lifecycle test; run it only against an isolated disposable tmux server. The standard test suite skips that test.

The [`website/`](website/) directory contains the documentation site source. Its Pages deployment is manual until GitHub Pages is enabled for this repository. No hosted documentation URL is required for installation.

## License

MIT. The original copyright notice is retained in [`LICENSE`](LICENSE).

## Credits

AgentsTmux is based on [tmux-agent-sidebar](https://github.com/hiroppy/tmux-agent-sidebar) by [hiroppy (Yuta Hiroto)](https://github.com/hiroppy). Thank you for the original sidebar, agent adapters, worktree tooling, and documentation. AgentsTmux is independently maintained by [ahmad9059](https://github.com/ahmad9059), with the single-window workspace, selection and ordering fixes, compact views, notification improvements, and additional integrations described above.
