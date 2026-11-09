import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, realpathSync, renameSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { ModApi } from "@commandcode/harness";

// Command Code evaluates mods with a synthetic .load-N filename for reloads.
export function resolveHookPath(moduleURL: string) {
  const source = fileURLToPath(moduleURL).replace(/\.load-\d+(?=\.[^.]+$)/, "");
  return join(dirname(realpathSync(source)), "../../hook.sh");
}

// Observe only. No hook changes prompts, permissions, results or agent state.
export default function (cmd: ModApi) {
  if (!process.env.TMUX_PANE) return;
  const hook = process.env.AGENTSTMUX_HOOK || resolveHookPath(import.meta.url);
  let session = "";
  let mode = "default";
  for (const [index, arg] of process.argv.entries()) {
    if (arg === "--permission-mode") mode = process.argv[index + 1] || mode;
    else if (arg.startsWith("--permission-mode=")) mode = arg.slice("--permission-mode=".length);
    else if (arg === "--plan") mode = "plan";
    else if (arg === "--accept-edits") mode = "auto-accept";
    else if (arg === "--yolo") mode = "bypass";
  }
  let queue = Promise.resolve();
  let failed = false;
  let prompt = "";
  const tools = new Map<string, { tool_name: string; tool_input: unknown }>();
  function emit(name: string, data = {}, quiet = false) {
    const payload = { cwd: process.cwd(), session_id: session, permission_mode: mode, ...data };
    queue = queue.then(() => new Promise<void>((resolve) => {
      const child = spawn("bash", [hook, "commandcode", name], {
        stdio: ["pipe", "ignore", "ignore"],
        env: { ...process.env, AGENTSTMUX_QUIET: quiet ? "1" : "0" },
      });
      child.on("error", () => resolve());
      child.on("close", () => resolve());
      child.stdin.on("error", () => {});
      child.stdin.end(JSON.stringify(payload));
    })).catch(() => {});
    return queue;
  }
  function metrics(model?: string, usage?: { inputTokens?: number; outputTokens?: number }) {
    if (!session) return;
    try {
      const file = join(homedir(), ".local/state/agentstmux/commandcode", `${encodeURIComponent(session)}.json`);
      let prior: any = {};
      try { prior = JSON.parse(readFileSync(file, "utf8")); } catch {}
      const data = {
        session_id: session, cwd: process.cwd(), model: model || cmd.getModel() || prior.model || "",
        input: (prior.input || 0) + (usage?.inputTokens || 0),
        output: (prior.output || 0) + (usage?.outputTokens || 0),
        requests: (prior.requests || 0) + (usage ? 1 : 0),
        updated_at: Date.now(),
      };
      mkdirSync(dirname(file), { recursive: true, mode: 0o700 });
      const temporary = `${file}.${process.pid}.tmp`;
      writeFileSync(temporary, JSON.stringify(data), { mode: 0o600 });
      renameSync(temporary, file);
    } catch { /* Monitoring must never interrupt the agent. */ }
  }
  cmd.hooks({
    onSessionStart: ({ source, sessionId }: any) => {
      session = sessionId || "";
      failed = false;
      metrics();
      void emit("session-start", { source });
    },
    onSessionEnd: ({ reason }) => emit("session-end", { end_reason: reason }),
    transformInput: ({ text }) => { prompt = text; return undefined; },
    onPermissionRequest: () => { void emit("notification", { notification_type: "permission_prompt" }); return undefined; },
  });
  cmd.on("run_start", (event) => {
    session = event.sessionId || session;
    failed = false;
    tools.clear();
    metrics();
    void emit("user-prompt-submit", { prompt });
    prompt = "";
  });
  cmd.on("permission_mode_changed", (event) => {
    mode = event.mode;
    void emit("notification", { notification_type: "idle_prompt" }, true);
  });
  cmd.on("model_request_start", (event) => {
    metrics(event.model);
    void emit("user-prompt-submit", {}, true);
  });
  cmd.on("model_request_end", (event) => { metrics(event.model, event.usage); });
  cmd.on("tool_queued", (event) => {
    tools.set(event.toolCallId, { tool_name: event.toolName, tool_input: event.input });
  });
  cmd.on("tool_running", (event) => {
    if (["ask_user", "ask_user_question"].includes(event.toolName)) {
      void emit("notification", { notification_type: "question:commandcode" });
    } else {
      void emit("user-prompt-submit", {}, true);
    }
  });
  cmd.on("tool_completed", (event) => {
    const tool = tools.get(event.toolCallId);
    tools.delete(event.toolCallId);
    if (tool) void emit("activity-log", { ...tool, tool_response: event.result }, true);
    void emit("user-prompt-submit", {}, true);
  });
  cmd.on("tool_errored", (event) => {
    const tool = tools.get(event.toolCallId);
    tools.delete(event.toolCallId);
    if (tool) void emit("activity-log", { ...tool, tool_response: { error: event.error } }, true);
  });
  cmd.on("tool_denied", () => { void emit("permission-denied"); });
  cmd.on("subagent_start", (event) => { void emit("subagent-start", { agent_type: event.subagentType, agent_id: event.toolCallId }); });
  cmd.on("subagent_stop", (event) => { void emit("subagent-stop", { agent_type: event.subagentType, agent_id: event.toolCallId }); });
  cmd.on("run_error", (event: any) => {
    failed = true;
    void emit("stop-failure", { error: event.error?.message || event.message || "Command Code request failed" });
  });
  cmd.on("run_end", (event) => {
    if (failed) return;
    if (event.result.stopReason === "permission_denied") {
      void emit("stop-failure", { error: "Permission denied" });
    } else {
      void emit("stop", { last_assistant_message: event.result.finalText || "" }, event.result.stopReason === "interrupted");
    }
  });
}
