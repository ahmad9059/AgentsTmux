# AgentsTmux

One tmux window for your coding agents. Keep a persistent sidebar on the left, switch live agents on the right, and leave the rest running in the background.

## Features

- **One Agents tab:** new agents use a detached holding session instead of adding tabs to your main session. Selecting an agent swaps its live pane into view; its process, conversation, scrollback, and worktree stay intact.
- **Stable ordering:** folders and the agents in them stay in creation order. A new folder or agent appears at the bottom instead of jumping around during swaps.
- **Reliable selection:** the visible agent stays highlighted while you type. Keyboard navigation has its own cursor, tracked by pane identity rather than an unstable row number.
- **Live usage panel:** the bottom panel shows the focused agent's own limits — Claude Code's 5-hour and weekly windows, Codex's primary and weekly windows with plan and credits — refreshed every 10 seconds and immediately when you switch agents. OpenCode and Pi are harnesses rather than subscriptions, so they report local session spend and reuse the subscription windows of the provider they are running on. When there is nothing to meter the panel says so instead of showing stale numbers.
- **Compact sidebar:** hide prompt/reply previews and the idle placeholder; retain status, branch, and task progress.
- **Useful notifications:** short, single-line completion, question, permission, and error alerts. Session selection does not replay old completions. On Hyprland, alerts are suppressed when the source tmux window is visible on an active workspace.
- **Live status indicators:** a yellow braille spinner while an agent works, a blue orbiting dot for background work, a blinking orange dot when it needs you, a green ring when it is ready, and a red dot on error. Every icon and color can be overridden.
- **Sound effects:** a soft chime when an agent finishes and a double ping when it asks a question, needs permission, or fails. Played only when you are not looking at that agent, like the desktop alerts.
- **Papirus status icons:** green completion tick, question, and error icons, with theme-icon fallbacks when Papirus is unavailable.
- **Claude Code, Codex, OpenCode V2, and Pi:** agent integrations are included, including the client-side bridge needed by OpenCode's shared-service architecture.
- **Start agents in any folder:** the **+** button opens an agent in the current folder, or in a path you type. No git worktree or branch is created. Closing an agent never touches its folder or kills the shared Agents window.
- **Quiet by default:** the sidebar appears only where you ask for it (**prefix + e**) and in windows named "Agents" or "Agent". It keeps its configured width when the terminal is resized.

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

That is all. The compact single-window profile is the default, and the first load wires up every agent it finds (see [Agent setup](#agent-setup)).

### Manual installation

```sh
git clone https://github.com/ahmad9059/AgentsTmux.git ~/.tmux/plugins/AgentsTmux
cd ~/.tmux/plugins/AgentsTmux
cargo build --release
```

Then add the launcher to `~/.tmux.conf` and reload:

```tmux
run-shell ~/.tmux/plugins/AgentsTmux/agents-tmux.tmux
```

## Agent setup

Nothing to run. Each time the plugin loads it runs `agents-tmux integrate --auto` in the background, which configures the agents it detects and skips the rest. It is idempotent, keeps your unrelated settings, and backs up any file it changes to `~/.local/state/agentstmux/backups/`.

- **Claude Code** (`claude` on `PATH`): installs `agents-tmux@agentstmux` from this repository's local marketplace. Start a new Claude session to load it.
- **Codex** (`~/.codex` exists): merges lifecycle hooks into `~/.codex/hooks.json`. Review and trust the new definitions with `/hooks`; hook trust is never bypassed.
- **OpenCode V2** (`~/.config/opencode` exists): links the bundled TUI plugin and adds it to `cli.json`. It runs in the local client so events target the correct `TMUX_PANE`, rather than the shared server's environment. Use `/reload` if OpenCode is already running.
- **Pi** (`~/.pi/agent` exists): links the bundled extension. Pi uses the Claude-compatible adapter and badge; its preview payloads are marked `[Pi]`.

To opt out, add `set -g @agentstmux_auto_setup off` before the plugin loads. To set up an agent that was not detected, or to run the step by hand, use `agents-tmux integrate --all` (or `--claude --codex --opencode --pi`).

The original V1-style OpenCode bridge is retained at `integrations/opencode-v1/agents-tmux.js`; do not load it in OpenCode V2.

When replacing an older sidebar installation, disable its plugin and hooks. AgentsTmux never removes differently named integrations from your settings; leaving both enabled can produce duplicate events.

For custom plugin locations, set `AGENTSTMUX_HOOK` to the absolute path of `hook.sh` before launching OpenCode or Pi.

## Usage

- **Prefix + e:** toggle the sidebar in the current window.
- **Prefix + E:** toggle sidebars across windows.
- **Click an agent**, or navigate with **j/k** or the arrow keys and press **Enter**, to show it in the right pane.
- Folders and agents are listed in creation order: a new folder goes to the bottom of the sidebar, and a new agent goes to the bottom of its folder.
- Use the project **+** button to start an agent. It opens a form in a popup centered over the window (Tab or ↑/↓ to move, ←/→ to change agent and mode, Esc to cancel). The folder field is prefilled with the current folder; edit it (or press **Ctrl+U** to clear it) to open the agent elsewhere. `~` and relative paths work, and the folder must already exist.
- `agents-tmux spawn [folder]` does the same from a shell.
- The bottom panel has two tabs, **Usage** and **Git**. Press **Shift+Tab** to switch, or click a tab name. Usage always follows the agent in view.
- When the agent in view exits (`/exit`, Ctrl+C) or is closed, the window switches to another agent instead of leaving you at a shell: first one in the same folder (the one above it, else below), otherwise the top agent of the nearest folder above (else below). The leftover shell of an agent started from the sidebar is closed.
- Use an agent's **×** button for removal options. Removing an agent does not close the shared host window.

The sidebar is never opened in every window automatically. It is created by hand with **prefix + e**, and automatically in windows named "Agents" or "Agent" (new, renamed, or restored ones). Set `@sidebar_auto_create` to `on` for every new window, or `off` for none. `@sidebar_width` sets the width in columns (default `32`) or as a percentage such as `15%`.

In single-window mode, other agents remain in a detached `__agent-sidebar-*` holding session. These are live panes, not suspended processes. Do not kill the holding session unless you intend to stop those agents.

### Sound effects

Sounds are on by default and follow the same rules as desktop notifications: only for events you have enabled, only when the agent is out of view, and never when an old session is merely re-selected. They are bundled in the binary and played with the first player found on your system (`pw-play`, `paplay`, `aplay`, `afplay`, `mpv`, `ffplay`, or `play`).

```tmux
set -g @sidebar_sound off                          # turn sounds off
set -g @sidebar_sound_done ~/sounds/finished.wav   # your own file for "finished"
set -g @sidebar_sound_request ~/sounds/ask.wav     # your own file for "needs you"
```

Run `agents-tmux play-sound done` or `agents-tmux play-sound request` to test your setup. The bundled sounds are synthesized by `scripts/generate-sounds.py`.

### Configuration

These are the built-in defaults, applied only when you have not set the option yourself. Put your own values in `~/.tmux.conf` before the plugin loads to override them ([`examples/agents-tmux.conf`](examples/agents-tmux.conf) lists them all). Set `AGENTSTMUX_SHOW_PREVIEWS 1` to show prompt/reply previews again.

```tmux
set -g @sidebar_single_window on
set -g @sidebar_icon_running '⟳'
set -g @sidebar_icon_idle '✓'
set -g @sidebar_color_idle '#4caf50'
set-environment -g AGENTSTMUX_SHOW_PREVIEWS 0
```

The first spawn from a sidebar records that window as its session's host. Existing agents in other windows are not silently migrated. Advanced users can adopt a pane with `agents-tmux workspace-select <host-window> <pane> <owner-session>`; existing markers are promoted to pane scope before swapping. Moving an existing window out of the main tab bar is a separate, explicit tmux operation.

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

AgentsTmux is based on [tmux-agent-sidebar](https://github.com/hiroppy/tmux-agent-sidebar) by [hiroppy (Yuta Hiroto)](https://github.com/hiroppy). Thank you for the original sidebar, agent adapters, and documentation. AgentsTmux is independently maintained by [ahmad9059](https://github.com/ahmad9059), with the single-window workspace, selection and ordering fixes, compact views, notification improvements, and additional integrations described above.
