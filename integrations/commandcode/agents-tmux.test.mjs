import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import bridge, { resolveHookPath } from "./agents-tmux.ts";

test("Command Code's synthetic reload filename resolves to the real hook", () => {
  const source = new URL("./agents-tmux.ts", import.meta.url);
  const synthetic = new URL("./agents-tmux.load-42.ts", import.meta.url);
  assert.equal(resolveHookPath(synthetic.href), resolveHookPath(source.href));
});

test("mod observes lifecycle and usage without changing permission decisions", async () => {
  const root = mkdtempSync(join(tmpdir(), "agentstmux-commandcode-"));
  const original = { ...process.env };
  try {
    process.env.HOME = root;
    process.env.TMUX_PANE = "%900";
    process.env.AGENTSTMUX_HOOK = join(root, "hook.sh");
    process.env.TEST_EVENTS = join(root, "events");
    writeFileSync(process.env.AGENTSTMUX_HOOK, 'printf "%s " "$2" >> "$TEST_EVENTS"\ncat >> "$TEST_EVENTS"\nprintf "\\n" >> "$TEST_EVENTS"\n');
    const observers = new Map();
    let hooks;
    bridge({ hooks(value) { hooks = value; }, on(name, fn) { observers.set(name, fn); }, getModel() { return "deepseek/test"; } });
    const emit = (name, value = {}) => { assert.ok(observers.has(name), name); observers.get(name)(value); };
    hooks.onSessionStart({ source: "startup", sessionId: "cmd-1" });
    assert.equal(hooks.transformInput({ text: "do work" }), undefined);
    emit("run_start", { sessionId: "cmd-1" });
    assert.equal(hooks.onPermissionRequest({ toolName: "shell_command" }), undefined);
    emit("tool_queued", { toolCallId: "t1", toolName: "shell_command", input: { command: "pwd" } });
    emit("tool_running", { toolCallId: "t1", toolName: "shell_command" });
    emit("tool_completed", { toolCallId: "t1", result: "work" });
    emit("permission_mode_changed", { mode: "plan" });
    emit("model_request_start", { model: "deepseek/test" });
    emit("model_request_end", { model: "deepseek/test", usage: { inputTokens: 100, outputTokens: 20 } });
    emit("subagent_start", { toolCallId: "child", subagentType: "explore" });
    emit("subagent_stop", { toolCallId: "child", subagentType: "explore" });
    emit("run_end", { result: { finalText: "done" } });
    await hooks.onSessionEnd({ reason: "shutdown" });
    const events = readFileSync(process.env.TEST_EVENTS, "utf8").trim().split("\n").map(line => {
      const split = line.indexOf(" ");
      return [line.slice(0, split), JSON.parse(line.slice(split + 1))];
    });
    assert.deepEqual(events.map(([name]) => name), [
      "session-start", "user-prompt-submit", "notification", "user-prompt-submit", "activity-log",
      "user-prompt-submit", "notification", "user-prompt-submit", "subagent-start", "subagent-stop", "stop", "session-end",
    ]);
    assert.equal(events[1][1].prompt, "do work");
    assert.equal(events[2][1].notification_type, "permission_prompt");
    assert.ok(events.every(([, value]) => value.session_id === "cmd-1"));
    const usage = JSON.parse(readFileSync(join(root, ".local/state/agentstmux/commandcode/cmd-1.json"), "utf8"));
    assert.equal(usage.input, 100);
    assert.equal(usage.output, 20);
    assert.equal(usage.requests, 1);
    // A failed run must not be overwritten by a subsequent success event.
    emit("run_error", { error: new Error("failed") });
    emit("run_end", { result: { finalText: "" } });
    await hooks.onSessionEnd({ reason: "shutdown" });
    const tail = readFileSync(process.env.TEST_EVENTS, "utf8").trim().split("\n").slice(-2);
    assert.ok(tail[0].startsWith("stop-failure "));
    assert.ok(tail[1].startsWith("session-end "));
    emit("run_start", { sessionId: "cmd-1" });
    emit("tool_running", { toolCallId: "question", toolName: "ask_user_question" });
    emit("run_end", { result: { stopReason: "permission_denied" } });
    await hooks.onSessionEnd({ reason: "shutdown" });
    const denied = readFileSync(process.env.TEST_EVENTS, "utf8").trim().split("\n").slice(-3);
    assert.equal(JSON.parse(denied[0].slice(denied[0].indexOf(" ") + 1)).notification_type, "question:commandcode");
    assert.ok(denied[1].startsWith("stop-failure "));
  } finally {
    process.env = original;
  }
});

test("outside tmux the mod registers nothing", () => {
  const pane = process.env.TMUX_PANE;
  delete process.env.TMUX_PANE;
  try { bridge({ hooks() { assert.fail("registered outside tmux"); } }); }
  finally { if (pane !== undefined) process.env.TMUX_PANE = pane; }
});
