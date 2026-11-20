// Run from the repository root with Node 22+, Chromium/Brave and util-linux script:
// node crates/techne-cli/tests/web-smoke.mjs target/debug/techne brave /tmp/techne-web.png
import assert from "node:assert/strict";
import {spawn} from "node:child_process";
import {mkdtemp, mkdir, readFile, rm, writeFile} from "node:fs/promises";
import {get} from "node:http";
import {tmpdir} from "node:os";
import {join, resolve} from "node:path";
import {setTimeout as sleep} from "node:timers/promises";

const [, , binary = "target/debug/techne", browser = "chromium", image] = process.argv;
const exe = resolve(binary);
const root = await mkdtemp(join(tmpdir(), "techne-web-test-"));
const children = [];
function start(program, args, env) {
  const p = spawn(program, args, {env, stdio: ["pipe", "pipe", "pipe"]});
  p.output = "";
  p.stdout.on("data", (bytes) => { p.output = (p.output + bytes).slice(-100_000); });
  p.errors = "";
  p.stderr.on("data", (bytes) => { p.errors = (p.errors + bytes).slice(-8000); });
  p.stdin.on("error", () => {});
  children.push(p);
  p.on("error", (e) => { p.output = e.message; });
  return p;
}
async function until(check, description) {
  const end = Date.now() + 20_000;
  while (Date.now() < end) {
    const result = await check();
    if (result) return result;
    await sleep(70);
  }
  throw new Error(`Timed out: ${description}`);
}
function refuse(url, origin, protocols, host) {
  return new Promise((resolve, reject) => {
    const request = get(new URL("/attach", url), {headers: {
      Host: host, Origin: origin, Connection: "Upgrade", Upgrade: "websocket",
      "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==", "Sec-WebSocket-Version": "13", "Sec-WebSocket-Protocol": protocols,
    }}, (r) => { r.resume(); resolve(r.statusCode); });
    request.on("upgrade", (_, stream) => { stream.destroy(); resolve(101); });
    request.on("error", reject);
  });
}

let socket;
try {
  const run = join(root, "run");
  await mkdir(run, {mode: 0o700});
  const env = {...process.env, SHELL: "sh", XDG_RUNTIME_DIR: run, XDG_STATE_HOME: join(root, "state"), XDG_CONFIG_HOME: join(root, "config"), TERM: "xterm-256color"};
  const file = join(root, "shared.scm");
  await writeFile(file, '\n(import (techne))\n\n;; Same faces and layout as the desktop editor.\n(define (greet name)\n  (string-append "Hello, " name))\n\n(greet "Techne")\n');
  start(exe, ["daemon"], env);
  async function web(file) {
    const gateway = start(exe, ["web", file], env);
    return until(() => gateway.output.match(/http:\/\/127\.0\.0\.1:\d+\/#[a-f0-9]{64}/)?.[0], "gateway URL");
  }
  const url = await web(file);
  const u = new URL(url);
  const protocols = `techne-v3, ${u.hash.slice(1)}`;
  assert.equal(await refuse(url, "http://other.example", protocols, u.host), 403);
  assert.equal(await refuse(url, u.origin, "techne-v3, wrong", u.host), 403);
  assert.equal(await refuse(url, u.origin, protocols, "other.example"), 403);
  assert.equal((await fetch(new URL("/eval", url))).status, 404);
  const browserDir = join(root, "browser");
  start(browser, ["--headless", "--disable-gpu", "--window-size=1200,850", "--remote-debugging-port=0", `--user-data-dir=${browserDir}`, "about:blank"], process.env);
  const port = await until(async () => (await readFile(join(browserDir, "DevToolsActivePort"), "utf8").catch(() => "")).split("\n")[0], "browser debugging port");
  const [page] = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, {once: true});
    socket.addEventListener("error", reject, {once: true});
  });
  let seq = 0;
  const waiting = new Map();
  socket.addEventListener("message", ({data}) => {
    const reply = JSON.parse(data);
    const next = waiting.get(reply.id);
    if (!next) return;
    waiting.delete(reply.id);
    if (reply.error) next.reject(new Error(reply.error.message));
    else next.resolve(reply.result);
  });
  function cdp(method, params = {}) {
    const id = ++seq;
    return new Promise((resolve, reject) => {
      waiting.set(id, {resolve, reject});
      socket.send(JSON.stringify({id, method, params}));
    });
  }
  async function dom(expression) {
    const reply = await cdp("Runtime.evaluate", {expression, returnByValue: true});
    if (reply.exceptionDetails) throw new Error(reply.exceptionDetails.exception?.description ?? reply.exceptionDetails.text);
    return reply.result.value;
  }
  const active = 'document.querySelector("[aria-label=\\"Active buffer\\"]")';
  const connected = 'document.querySelector("#connection[data-live]") !== null';
  await cdp("Page.navigate", {url});
  // The about:blank document and the new page can briefly overlap navigation.
  await until(() => dom(connected), "browser connection");
  async function key(key, code, modifiers = 0) {
    await cdp("Input.dispatchKeyEvent", {type: "keyDown", key, code, modifiers, ...(!modifiers && [...key].length === 1 ? {text: key} : {})});
    await cdp("Input.dispatchKeyEvent", {type: "keyUp", key, code, modifiers});
  }
  const text = `${active}?.querySelector(".source")?.textContent`;
  assert.equal(await dom("location.hash"), "", "secret removed from address bar");
  await key("x", "KeyX");
  await until(() => dom(`${text} === "x"`), "browser key reaches editor");
  await cdp("Input.insertText", {text: " café 🦀"});
  await until(() => dom(`${text} === "x café 🦀"`), "browser Unicode insertion reaches editor");
  const quoted = (value) => "'" + value.replaceAll("'", "'\\''") + "'";
  const terminal = start("script", ["-qfec", `stty rows 24 cols 100; exec ${quoted(exe)} term --attach ${quoted(file)}`, "/dev/null"], env);
  await until(() => {
    if (terminal.exitCode !== null) throw new Error(`Native terminal exited: ${terminal.output} ${terminal.errors}`);
    return terminal.output.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "").includes("caf");
  }, "native terminal sees browser edits");
  terminal.stdin.write("!");
  await until(() => dom(`${text}?.includes("!")`), "browser sees native terminal edits");
  const paste = () => dom(`(() => {
    const data = new DataTransfer();
    data.setData("text/plain", "same paste");
    document.getElementById("keys").dispatchEvent(new ClipboardEvent("paste", {clipboardData: data, bubbles: true, cancelable: true}));
  })()`);
  await paste();
  await until(() => dom(`${text}?.includes("same paste")`), "browser pastes explicit text");
  terminal.stdin.write("\x01\x0b"); // C-a C-k in the native frontend changes the shared kill ring.
  await until(() => dom(`${text} === ""`), "native terminal kills the line");
  await paste();
  await until(() => dom(`${text} === "same paste"`), "repeat paste ignores another frontend's kill");
  const shared = await dom(text);
  await cdp("Page.reload");
  await until(() => dom(`${connected} && ${text} === ${JSON.stringify(shared)}`), "refresh preserves shared buffer");
  await key("x", "KeyX", 2);
  await key("s", "KeyS", 2);
  await until(async () => (await readFile(file, "utf8")).split("\n")[0] === shared, "browser saves shared buffer");
  await until(() => dom('Array.from(document.querySelectorAll(".source .text")).some(n => n.textContent === "define" && n.style.fontWeight === "700")'), "Scheme face appears in browser DOM");
  await key("x", "KeyX", 2);
  await key("3", "Digit3");
  await until(() => dom('document.querySelectorAll(".pane").length === 2'), "editor splits the browser frame");
  assert.equal(await dom('document.querySelectorAll(".pane")[1].style.left'), "50%");
  await key("x", "KeyX", 1);
  await until(() => dom('!document.getElementById("minibuffer").hidden && document.querySelectorAll("[role=option]").length > 0'), "desktop minibuffer and candidates render");
  for (const c of "scree") await key(c, `Key${c.toUpperCase()}`);
  await until(() => dom('document.getElementById("command").textContent === "scree" && document.querySelector("#choices [role=option]")?.textContent.includes("screenshot")'), "minibuffer filters candidates as you type");
  if (image) {
    const shot = await cdp("Page.captureScreenshot", {format: "png"});
    await writeFile(image, Buffer.from(shot.data, "base64"));
  }
  // Committed text reaches a terminal's child as a paste, not as an edit
  // of its screen; the child sees the browser's reported size.
  await key("g", "KeyG", 2);
  await key("x", "KeyX", 1);
  for (const c of "terminal") await key(c, `Key${c.toUpperCase()}`);
  await key("Enter", "Enter");
  await until(() => dom(`${active}?.closest(".pane")?.querySelector(".mode")?.textContent.includes("terminal")`), "a terminal opens in the browser");
  await cdp("Input.insertText", {text: "echo pasted-$((6*7)); stty size"});
  await key("Enter", "Enter");
  await until(() => dom(`${active}?.textContent.includes("pasted-42")`), "browser text reaches the terminal's child");
  const [rows, columns] = (await until(() => dom(`snapshot.panes[snapshot.focus].text.match(/^(\\d+) (\\d+) *$/m)?.slice(1)`), "the child reports its size")).map(Number);
  assert.ok(columns > 20 && columns < 200 && rows > 5, `terminal fitted to the browser viewport: ${rows}x${columns}`);
  if (image) {
    const shot = await cdp("Page.captureScreenshot", {format: "png"});
    await writeFile(image.replace(/(\.png)?$/, "-terminal.png"), Buffer.from(shot.data, "base64"));
  }
  const wrapped = join(root, "wrapped.txt");
  await writeFile(wrapped, "some text with spaces ".repeat(800));
  await cdp("Page.navigate", {url: await web(wrapped)});
  await until(() => dom(`${connected} && ${text}?.startsWith("some text")`), "one heavily wrapped logical line is shown");
  const head = "snapshot.panes[snapshot.focus].selections[0][1]";
  await key("PageDown", "PageDown");
  const firstPage = await until(() => dom(`${head} > 0 && ${head}`), "Page Down advances within the wrapped line");
  await key("PageDown", "PageDown");
  await until(() => dom(`${head} > ${firstPage}`), "a second Page Down advances within the same line");
  await key("PageUp", "PageUp");
  await until(() => dom(`${head} > 0 && ${head} < ${firstPage + 100}`), "Page Up moves back within the wrapped line");
  await dom(`(() => {
    const item = [...panes.values()][0];
    const anchor = snapshot.panes[0].scroll;
    item.buffer.scrollTop += 160;
    item.buffer.dispatchEvent(new Event("scroll"));
    render({...snapshot, panes: snapshot.panes.map(p => ({...p}))});
    window.anchorBeforeWheel = anchor;
  })()`);
  await until(() => dom("snapshot.panes[0].scroll > window.anchorBeforeWheel"), "wheel scroll survives an unrelated snapshot and sends a visual anchor");

  // Model the UI's interval after a restart notice and before the first
  // new presentation. The Rust attachment test also crashes a real runtime
  // and checks that an old-generation input is rejected at the daemon.
  assert.equal(await dom(`(() => {
    clipboard = "old clipboard";
    invalidate();
    return snapshot === undefined && clipboard === "" && !key("x");
  })()`), true, "input and clipboard are cleared between runtime generations");

  const dense = join(root, "dense.txt");
  await writeFile(dense, "\n".repeat(500_000));
  await cdp("Page.navigate", {url: await web(dense)});
  await until(() => dom('document.getElementById("connection-message")?.textContent.includes("size limit")'), "row budget rejects a dense buffer below the byte limit");
  assert.equal(await dom('document.querySelectorAll(".row").length'), 0);
  console.log("Web smoke passed: endpoint guards, Unicode, shared edits and paste, saving, faces, splits, minibuffer, wrapped paging and wheel scroll, terminal paste and size, dense buffer budget.");
} finally {
  socket?.close();
  const ended = children.map((p) => new Promise((resolve) => {
    if (p.exitCode !== null || p.signalCode !== null || p.pid === undefined) resolve();
    else { p.once("exit", resolve); p.kill(); }
  }));
  await Promise.race([Promise.all(ended), sleep(2500)]);
  for (const p of children) {
    if (p.exitCode === null && p.signalCode === null) p.kill("SIGKILL");
    // Descendants may briefly inherit these pipes after their parent exits.
    p.stdin.destroy();
    p.stdout.destroy();
    p.stderr.destroy();
    p.unref();
  }
  await rm(root, {recursive: true, force: true, maxRetries: 5, retryDelay: 100});
}
