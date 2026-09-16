// The forms, checked without a browser: form.js loaded with a stub
// document, and every arm, `save`, `priority`, route and `test_`
// function of the examples read into a form and written back — the
// lines must come back as they were, or the form must have said no.
// Run: node editors/web/test/forms.js [file.rill ...]

const fs = require("fs");
const path = require("path");
const vm = require("vm");

const root = path.join(__dirname, "../../..");
const sandbox = { document: { addEventListener() {}, createElement() { throw new Error("no DOM here"); } }, window: {} };
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(path.join(__dirname, "../static/form.js"), "utf8") + "\nthis.Forms = Forms;", sandbox);
const Forms = sandbox.Forms;

let checks = 0, failures = 0;
const fail = (what, got, want) => { failures++; console.log(`FAIL ${what}\n  got:  ${JSON.stringify(got)}\n  want: ${JSON.stringify(want)}`); };
const same = (what, got, want) => { checks++; if (got !== want) fail(what, got, want); };

// The last line of the block that starts at `line` (1-based), by indentation.
function blockEnd(ls, line) {
  const indent = l => l.match(/^ */)[0].length;
  const base = indent(ls[line - 1]);
  let end = line;
  for (let i = line; i < ls.length; i++) {
    if (ls[i].trim() === "") continue;
    if (indent(ls[i]) <= base) break;
    end = i + 1;
  }
  return end;
}

function roundTrips(file) {
  const text = fs.readFileSync(file, "utf8");
  const ls = text.split("\n");
  let inMachine = false, inSystem = false, inState = false;
  for (let i = 0; i < ls.length; i++) {
    const l = ls[i];
    if (/^machine /.test(l)) { inMachine = true; inSystem = false; }
    else if (/^system /.test(l)) { inSystem = true; inMachine = false; }
    else if (/^\S/.test(l) && l.trim() !== "") { inMachine = false; inSystem = false; }
    if (inMachine && /^  state /.test(l)) inState = true;
    else if (inMachine && /^  \S/.test(l)) inState = false;
    const where = `${path.relative(root, file)}:${i + 1}`;
    // an arm, a save or a priority: exactly four spaces in, inside a state
    if (inMachine && inState && /^    \S/.test(l)) {
      const end = blockEnd(ls, i + 1);
      const lines = ls.slice(i, end);
      const arm = Forms.parse(lines);
      if (arm) same(`arm ${where}`, Forms.write(arm), lines.join("\n"));
      i = end - 1;
      continue;
    }
    // a route
    if (inSystem && /^  \S/.test(l)) {
      const m = l.match(/^  ([a-z_][a-z0-9_]*) (->|<->) ([a-z_][a-z0-9_]*): (.+)$/);
      if (m) {
        // the route form has no parse — its model comes from `rill diagram --json`; the writer's shape is the file's
        const written = `  ${m[1]} ${m[2]} ${m[3]}: ${m[4].split(",").map(s => s.trim()).join(", ")}`;
        same(`route ${where}`, written, l);
      }
      continue;
    }
    // a scenario
    if (/^fn test_/.test(l)) {
      const end = blockEnd(ls, i + 1);
      const lines = ls.slice(i, end);
      const sc = Forms.parseScenario(lines);
      if (sc) {
        // a spawn's arguments are read by position and written by parameter name
        const machines = Forms.machinesOf(text);
        for (const st of sc.steps) if (st.kind === "spawn" && st.argv) { const mc = machines.find(x => x.name === st.machine); if (mc) mc.params.slice(1).forEach((p, k) => { st.args[p.name] = st.argv[k]; }); }
        same(`scenario ${where}`, Forms.writeScenario(sc, machines), lines.join("\n"));
      }
      i = end - 1;
    }
  }
}

// What the forms write from what they hold.
function writers() {
  same("an arm with actions", Forms.write({ kind: "input", input: "Answer", guard: "", ms: "", cond: "", actions: [{ kind: "output", to: "caller", signal: "Answer" }], end: { kind: "next", text: "Talking(caller)" } }),
    "    Answer ->\n      output(caller, Answer)\n      Talking(caller)");
  same("a guarded arm on one line", Forms.write({ kind: "input", input: "Digit(n)", guard: "n >= 0", actions: [], end: { kind: "next", text: "Dialling(Cons(n, digits))" } }),
    "    Digit(n) if n >= 0 -> Dialling(Cons(n, digits))");
  same("an after arm", Forms.write({ kind: "after", ms: "50", actions: [{ kind: "send", to: "log", text: '"gave up"' }], end: { kind: "next", text: "Idle" } }),
    "    after 50 ->\n      send(log, \"gave up\")\n      Idle");
  same("a when arm that stops", Forms.write({ kind: "when", cond: "len(items) >= 3", actions: [], end: { kind: "stop" } }), "    when len(items) >= 3 -> stop");
  same("a done", Forms.write({ kind: "input", input: "Yes", guard: "", actions: [{ kind: "reset", timer: "Retry" }], end: { kind: "done", text: "true" } }),
    "    Yes ->\n      reset(Retry)\n      done(true)");
  same("a save", Forms.write({ kind: "save", input: "Send(_)", guard: "" }), "    save Send(_)");
  same("a priority with a guard", Forms.write({ kind: "priority", input: "Lose(_)", guard: "tries > 2" }), "    priority Lose(_) if tries > 2");
  same("a set timer", Forms.write({ kind: "input", input: "Go", guard: "", actions: [{ kind: "set", timer: "Retry", ms: "100" }, { kind: "reply", signal: 'Status("going")' }], end: { kind: "stay" } }),
    "    Go ->\n      set(Retry, 100)\n      reply(Status(\"going\"))\n      stay");
  // what the form refuses: a body with a decision, a binding
  same("an if in the body is not a form", Forms.parse(["    Frame(bit, text, back) ->", "      if losing > 0 then stay else Idle"]), null);
  same("a binding is not a form", Forms.parse(["    Digit(n) ->", "      m = n + 1", "      Dialling(Cons(m, digits))"]), null);
  same("a match in the body is not a form", Forms.parse(["    Report ->", "      match x", "        1 -> reply(A)", "        _ -> reply(B)", "      stay"]), null);
  // a constructor
  same("a ctor", JSON.stringify(Forms.parseCtor("Held(peer: Chan(Mail(Signal)), since: Int)")), JSON.stringify({ name: "Held", fields: [{ name: "peer", type: "Chan(Mail(Signal))" }, { name: "since", type: "Int" }] }));
  same("a bare ctor", JSON.stringify(Forms.parseCtor("Idle")), JSON.stringify({ name: "Idle", fields: [] }));
  same("not a ctor", Forms.parseCtor("state Idle"), null);
  // a scenario read back
  const sc = Forms.parseScenario(["fn test_a_line_answers() =", "  env = mailbox()", "  log = channel(16)", "  a = mailbox()", "  spawn line(a, log, Idle)", "  post(a, env, OffHook)", "  match recv(env).signal", "    Status(_) -> ()", "    _ -> assert(false)", "  sleep_ms(100)"]);
  same("a scenario's steps", sc && sc.steps.map(s => s.kind).join(" "), "spawn post expect wait");
  same("a scenario's name", sc && sc.name, "a_line_answers");
  same("a scenario with a loop is code", Forms.parseScenario(["fn test_x() =", "  env = mailbox()", "  for i in 1..3", "    post(a, env, Tick)"]), null);
  same("a scenario not started by the form is code", Forms.parseScenario(["fn test_x() =", "  n = start()", "  post(n.s, n.env, Send(\"one\"))"]), null);
  const machines = Forms.machinesOf("machine line(me: Chan(Mail(Signal)), log: Chan(Str))\nmachine medium(me: Chan(Mail(Signal)))\n");
  same("machines off the headers", JSON.stringify(machines), JSON.stringify([{ name: "line", params: [{ name: "me", type: "Chan(Mail(Signal))" }, { name: "log", type: "Chan(Str)" }] }, { name: "medium", params: [{ name: "me", type: "Chan(Mail(Signal))" }] }]));
  same("a scenario written", Forms.writeScenario({ name: "two_lines", steps: [
    { kind: "spawn", inst: "a", machine: "line", args: { log: "log" }, state: "Idle" },
    { kind: "spawn", inst: "b", machine: "line", args: { log: "log" }, state: "Idle" },
    { kind: "post", to: "a", signal: "Ring" }, { kind: "expect", signal: "Status(_)" }, { kind: "wait", ms: "5" }, { kind: "line", text: "println(recv(log))" }] }, machines),
    "fn test_two_lines() =\n  env = mailbox()\n  log = channel(16)\n  a = mailbox()\n  spawn line(a, log, Idle)\n  b = mailbox()\n  spawn line(b, log, Idle)\n  post(a, env, Ring)\n  match recv(env).signal\n    Status(_) -> ()\n    _ -> assert(false)\n  sleep_ms(5)\n  println(recv(log))");
}

const files = process.argv.length > 2 ? process.argv.slice(2) : fs.readdirSync(path.join(root, "examples")).filter(f => f.endsWith(".rill")).map(f => path.join(root, "examples", f));
for (const f of files) roundTrips(f);
writers();
console.log(`${checks} checks, ${failures} failed`);
process.exit(failures ? 1 : 0);
