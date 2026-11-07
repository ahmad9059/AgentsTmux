import { Plugin } from "@opencode/plugin/tui";
import { spawn } from "node:child_process";
import { homedir } from "node:os";

// Run in the TUI, not the shared server: TMUX_PANE belongs to this client.
export default Plugin.define({
  id: "local.agents-tmux",
  setup(ctx) {
    if (!process.env.TMUX_PANE) return;
    const script = process.env.AGENTSTMUX_HOOK || ctx.options?.hookPath || `${homedir()}/.tmux/plugins/AgentsTmux/hook.sh`;
    let previous = "";
    let sessionID = "";
    let primed = false;
    let lastIdleTime = 0;
    let queue = Promise.resolve();
    const seenTools = new Set();
    function emit(name, payload, quiet = false) {
      queue = queue.then(() => new Promise((resolve) => {
        const child = spawn("bash", [script, "opencode", name], {
          stdio: ["pipe", "ignore", "ignore"],
          env: { ...process.env, AGENTSTMUX_QUIET: quiet ? "1" : "0" },
        });
        child.on("error", resolve);
        child.on("close", resolve);
        child.stdin.on("error", () => {});
        child.stdin.end(JSON.stringify(payload));
      })).catch(() => {});
    }
    function sync() {
      const route = ctx.ui.router.current();
      if (route.type !== "session") return;
      const session = ctx.data.session.get(route.sessionID);
      if (!session) return;
      const payload = {
        session_id: route.sessionID,
        cwd: session.location?.directory ?? ctx.location?.directory ?? process.cwd(),
      };
      const changedSession = sessionID !== route.sessionID;
      if (changedSession) {
        sessionID = route.sessionID;
        previous = "";
        primed = false;
        seenTools.clear();
        emit("session-start", { ...payload, source: "startup" }, true);
      }
      const messages = ctx.data.session.message.list(sessionID) ?? [];
      const user = messages.findLast((m) => m.type === "user");
      const permissions = ctx.data.session.permission.list(sessionID) ?? [];
      const forms = ctx.data.session.form.list(sessionID, ctx.location) ?? [];
      const busy = (session.time.busy ?? 0) > (session.time.idle ?? 0);
      const idleTime = session.time.idle ?? 0;
      // Session selection and initial cache hydration are snapshots, not events.
      const newCompletion = !changedSession && idleTime > lastIdleTime;
      const state = forms.length ? "question" : permissions.length ? "permission" : busy ? "busy" : session.outcome === "failed" ? "error" : "idle";
      const pendingID = forms.map((form) => form.id).join(",") || permissions.map((permission) => permission.id).join(",");
      const fingerprint = `${sessionID}:${state}:${user?.id ?? ""}:${idleTime}:${pendingID}`;
      if (fingerprint !== previous) {
        previous = fingerprint;
        if (state === "question") emit("notification", { ...payload, wait_reason: `question:${pendingID}` }, changedSession);
        else if (state === "permission") emit("notification", { ...payload, wait_reason: "permission" }, changedSession);
        else if (state === "busy") emit("user-prompt-submit", { ...payload, prompt: user?.text ?? "" });
        else if (state === "error") emit("stop-failure", { ...payload, error: "OpenCode session failed" }, !newCompletion);
        else emit("stop", { ...payload, last_message: "" }, !newCompletion);
      }
      for (const message of messages.slice(-8)) {
        if (message.type !== "assistant") continue;
        for (const part of message.content ?? []) {
          if (part.type !== "tool" || !["completed", "error"].includes(part.state?.status)) continue;
          if (seenTools.has(part.id)) continue;
          seenTools.add(part.id);
          if (primed) emit("activity-log", {
            ...payload, tool_name: part.name,
            tool_input: part.state.input ?? {}, tool_response: part.state.content ?? [],
          });
        }
      }
      primed = true;
      lastIdleTime = idleTime;
    }
    const tick = () => { try { sync(); } catch { /* Never interrupt the TUI. */ } };
    tick();
    const timer = setInterval(tick, 750);
    return () => clearInterval(timer);
  },
});
