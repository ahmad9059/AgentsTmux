import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

test("OpenCode: new events notify, snapshots stay silent, questions deduplicate", async () => {
  const emitted = [];
  let tick;
  let route = "ses_first";
  let session = { time: { idle: 1 }, location: { directory: "/project" } };
  let messages = [];
  let permissions = [];
  let forms = [];
  const sandbox = {
    result: undefined,
    Plugin: { define: (definition) => definition },
    process: { env: { TMUX_PANE: "%test" } },
    homedir: () => "/home/test",
    setInterval: (fn) => { tick = fn; return 1; },
    clearInterval: () => {},
    spawn: (_command, args, options) => {
      let close;
      return {
        on: (name, fn) => { if (name === "close") close = fn; },
        stdin: { on: () => {}, end: (text) => {
          emitted.push({ event: args[2], payload: JSON.parse(text), quiet: options.env.AGENTSTMUX_QUIET });
          close();
        } },
      };
    },
  };
  const source = readFileSync(new URL("../integrations/opencode-v2/tui.js", import.meta.url), "utf8")
    .replace(/^import .*;\n/gm, "").replace("export default ", "result = ");
  vm.createContext(sandbox);
  vm.runInContext(source, sandbox);
  const dispose = sandbox.result.setup({
    options: {},
    ui: { router: { current: () => ({ type: "session", sessionID: route }) } },
    data: { session: {
      get: () => session,
      message: { list: () => messages },
      permission: { list: () => permissions },
      form: { list: () => forms },
    } },
  });
  const flush = () => new Promise((resolve) => setImmediate(resolve));
  await flush();
  assert.equal(emitted.at(-1).quiet, "1", "initial idle snapshot must be silent");

  session = { ...session, time: { busy: 2, idle: 1 } };
  messages = [{ type: "user", id: "msg_one", text: "help" }];
  tick(); await flush();
  assert.equal(emitted.at(-1).event, "user-prompt-submit");
  forms = [{ id: "frm_one" }];
  tick(); await flush();
  assert.equal(emitted.at(-1).payload.wait_reason, "question:frm_one");
  assert.equal(emitted.at(-1).quiet, "0");
  const count = emitted.length;
  tick(); await flush();
  assert.equal(emitted.length, count, "unchanged question must not repeat");
  forms = [{ id: "frm_two" }];
  tick(); await flush();
  assert.equal(emitted.at(-1).payload.wait_reason, "question:frm_two");

  route = "ses_existing_question";
  tick(); await flush();
  assert.equal(emitted.at(-1).quiet, "1", "selecting an existing question must be silent");
  forms = [];
  permissions = [{ id: "permission_one" }];
  tick(); await flush();
  assert.equal(emitted.at(-1).payload.wait_reason, "permission");
  permissions = [];
  session = { ...session, time: { busy: 2, idle: 3 } };
  tick(); await flush();
  assert.equal(emitted.at(-1).event, "stop");
  assert.equal(emitted.at(-1).quiet, "0", "new completion should notify");
  assert.equal(emitted.at(-1).payload.last_message, "", "do not publish a reply preview");
  route = "ses_old_completion";
  tick(); await flush();
  assert.equal(emitted.at(-1).quiet, "1", "switching to old completion must be silent");
  dispose();
});
