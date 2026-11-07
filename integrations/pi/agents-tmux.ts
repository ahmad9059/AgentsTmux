import { spawn } from "node:child_process";
import { homedir } from "node:os";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

// Upstream has no Pi agent type. Use its Claude adapter and prefix previews [Pi].
export default function (pi: ExtensionAPI) {
  const tools = new Map<string, unknown>();
  let lastMessage = "";
  let failed = false;
  let running = false;
  let queue = Promise.resolve();
  function emit(event: string, ctx: ExtensionContext, extra = {}) {
    if (!process.env.TMUX_PANE) return;
    const payload = { cwd: ctx.cwd, session_id: ctx.sessionManager.getSessionId(), ...extra };
    queue = queue.then(() => new Promise<void>((resolve) => {
      const child = spawn("bash", [
        process.env.AGENTSTMUX_HOOK || `${homedir()}/.tmux/plugins/AgentsTmux/hook.sh`, "claude", event,
      ], { stdio: ["pipe", "ignore", "ignore"] });
      child.on("error", () => resolve());
      child.on("close", () => resolve());
      child.stdin.on("error", () => {});
      child.stdin.end(JSON.stringify(payload));
    })).catch(() => {});
    return queue;
  }
  pi.on("session_start", (_event, ctx) => {
    tools.clear();
    return emit("session-start", ctx, { source: "startup" });
  });
  pi.on("before_agent_start", (event, ctx) => {
    running = true;
    failed = false;
    lastMessage = "";
    return emit("user-prompt-submit", ctx, { prompt: `[Pi] ${event.prompt}` });
  });
  pi.on("tool_execution_start", (event) => { tools.set(event.toolCallId, event.args); });
  pi.on("tool_execution_end", (event, ctx) => {
    const names: Record<string, string> = { bash: "Bash", read: "Read", write: "Write", edit: "Edit", grep: "Grep", find: "Glob" };
    const args: any = tools.get(event.toolCallId) ?? {};
    tools.delete(event.toolCallId);
    return emit("activity-log", ctx, {
      tool_name: names[event.toolName] ?? event.toolName,
      tool_input: { ...args, file_path: args.path ?? args.file_path },
      tool_response: event.result,
    });
  });
  pi.on("message_end", (event) => {
    if (event.message.role !== "assistant") return;
    lastMessage = event.message.content.filter((p) => p.type === "text").map((p) => p.text).join("\n");
    failed = event.message.stopReason === "error";
  });
  pi.on("ui_prompt_start", (_event, ctx) => emit("notification", ctx, { notification_type: "permission_prompt" }));
  pi.on("ui_prompt_end", (_event, ctx) => {
    if (running) return emit("user-prompt-submit", ctx, { prompt: "" });
  });
  pi.on("agent_settled", (_event, ctx) => {
    running = false;
    return emit(failed ? "stop-failure" : "stop", ctx, {
      last_assistant_message: `[Pi] ${lastMessage}`, error: failed ? "Pi model request failed" : "",
    });
  });
  pi.on("session_shutdown", (_event, ctx) => emit("session-end", ctx, { reason: "other" }));
}
