"use strict";

const get = (id) => document.getElementById(id);
const make = (tag, css) => Object.assign(document.createElement(tag), {className: css});
const frame = get("frame"), keys = get("keys"), panes = new Map();
const encoder = new TextEncoder(), decoder = new TextDecoder("utf-8", {ignoreBOM: true});
const bytes = (text) => encoder.encode(text).length;
let socket, snapshot, clipboard = "", composing = false;
let token = location.hash.slice(1);
if (/^[0-9a-f]{64}$/.test(token)) {
  sessionStorage.setItem("techne-token", token);
  history.replaceState(null, "", location.pathname);
} else token = sessionStorage.getItem("techne-token");

function status(text, live = false) {
  const box = get("connection");
  box.hidden = live;
  box.toggleAttribute("data-live", live);
  get("connection-message").textContent = text;
}

function send(message) {
  if (socket?.readyState !== WebSocket.OPEN || snapshot === undefined) return false;
  const value = JSON.stringify({generation: snapshot.generation, input: message});
  if (bytes(value) > 256 * 1024) {
    get("echo").textContent = "Input is too large for one message.";
    return false;
  }
  if (socket.bufferedAmount > 256 * 1024) {
    socket.close(4001, "Frontend input stalled");
    status("Input stalled. Reconnect to continue.");
    return false;
  }
  socket.send(value);
  return true;
}

const key = (key) => send({type: "key", key});
const insertText = (text) => { if (text) send({type: "text", text}); };
function handleKey(event) {
  if (event.isComposing || composing || event.metaKey || event.getModifierState("AltGraph")) return;
  // Browser clipboard shortcuts still work where the browser delivers them.
  if (event.ctrlKey && event.shiftKey && !event.altKey && event.key.toLowerCase() === "v") return;
  if (event.ctrlKey && event.key.toLowerCase() === "c" && (!getSelection()?.isCollapsed || event.shiftKey)) return;
  const names = {Enter: "RET", Backspace: "DEL", Delete: "<delete>", Tab: "TAB", Escape: "ESC", " ": "SPC",
    ArrowUp: "<up>", ArrowDown: "<down>", ArrowLeft: "<left>", ArrowRight: "<right>", Home: "<home>", End: "<end>",
    PageUp: "<prior>", PageDown: "<next>", Insert: "<insert>"};
  const base = names[event.key] || (/^F(?:[1-9]|1[0-2])$/.test(event.key) ? `<${event.key.toLowerCase()}>` :
    [...event.key].length === 1 ? event.key : "");
  if (!base) return;
  event.preventDefault();
  const modified = event.ctrlKey || event.altKey;
  const shift = event.shiftKey && (modified || base.startsWith("<") || base === "TAB");
  key(`${event.ctrlKey ? "C-" : ""}${event.altKey ? "M-" : ""}${shift ? "S-" : ""}${modified && base.length === 1 ? base.toLowerCase() : base}`);
}
frame.addEventListener("keydown", (event) => { if (event.target !== get("reconnect")) handleKey(event); });
frame.addEventListener("paste", (event) => {
  event.preventDefault();
  insertText(event.clipboardData?.getData("text/plain"));
});
frame.addEventListener("copy", (event) => {
  const text = getSelection()?.toString() || clipboard;
  if (!text) return;
  event.clipboardData.setData("text/plain", text);
  event.preventDefault();
});
keys.addEventListener("compositionstart", () => { composing = true; });
keys.addEventListener("compositionend", (event) => {
  composing = false;
  insertText(event.data);
  keys.value = "";
});
keys.addEventListener("beforeinput", (event) => {
  if (composing || event.isComposing || !event.cancelable) return;
  event.preventDefault();
  if (event.inputType === "insertText") insertText(event.data);
  else if (event.inputType === "deleteContentBackward") key("DEL");
  else if (event.inputType === "insertLineBreak") key("RET");
});
keys.addEventListener("input", () => { if (!composing) keys.value = ""; });

const color = (c) => c ? `rgb(${c.rgb.join(",")})` : "";
const face = (id) => snapshot.styles[id] || {};
const chrome = (name) => face(snapshot.chrome[name]);
const merge = (...styles) => Object.assign({}, ...styles.map((s) => Object.fromEntries(Object.entries(s).filter(([, v]) => v != null))));
function paint(node, f, relative = true) {
  // Properties are set individually from structured faces, not via a style
  // attribute or interpolated HTML. A buffer is never executable markup.
  const s = node.style;
  s.color = color(f.fg);
  s.backgroundColor = color(f.bg);
  s.fontWeight = {Light: "300", Normal: "400", Bold: "700"}[f.weight] || "";
  s.fontStyle = f.slant === "Italic" ? "italic" : f.slant === "Normal" ? "normal" : "";
  s.fontFamily = f.family === "fixed" ? "monospace" : f.family === "variable" ? "system-ui" : f.family || "";
  s.fontSize = f.height ? `${relative ? f.height : f.height * .15}${relative ? "%" : "px"}` : "";
  const under = f.underline?.[0];
  s.textDecorationLine = [under && under !== "None" ? "underline" : "", f.strike ? "line-through" : ""].filter(Boolean).join(" ") || "none";
  s.textDecorationStyle = {Curl: "wavy", Dotted: "dotted", Dashed: "dashed", Double: "double"}[under] || "solid";
  s.textDecorationColor = color(f.underline?.[1]);
}
function caret(active, shape = "Bar") {
  if (shape === "Hidden") return null;
  const node = make("span", `caret${shape === "Block" ? " block" : ""}`);
  const f = chrome(active ? "cursor" : "cursor_inactive");
  node.style.color = color(f.bg || f.fg);
  node.setAttribute("aria-hidden", "true");
  return node;
}

// Positions crossing the wire are UTF-8 byte offsets; browser text nodes
// use UTF-16. Index once per revision so cursor movement does not rescan text.
function index(text) {
  let from = 0;
  return text.split("\n").map((line) => {
    const encoded = encoder.encode(line);
    const row = {from, to: from + encoded.length, encoded};
    from = row.to + 1;
    return row;
  });
}
function lineAt(lines, byte) {
  let lo = 0, hi = lines.length;
  while (lo + 1 < hi) {
    const mid = (lo + hi) >>> 1;
    if (lines[mid].from <= byte) lo = mid;
    else hi = mid;
  }
  return lo;
}
const slice = (line, start, end) => decoder.decode(line.encoded.subarray(start - line.from, end - line.from));
const offset = (item, node, units) => {
  const element = node.nodeType === Node.TEXT_NODE ? node.parentElement : node;
  const span = element.closest?.("[data-at]");
  if (span && item.buffer.contains(span)) return Number(span.dataset.at) + (node.nodeType === Node.TEXT_NODE && !span.classList.contains("inlay") ? bytes(node.data.slice(0, units)) : 0);
  const row = element.closest?.(".row[data-line]");
  return row ? item.lines[Number(row.dataset.line)].to : null;
};
function hit(item, x, y) {
  const rect = item.buffer.getBoundingClientRect();
  const boundedX = Math.min(rect.right - 2, Math.max(rect.left + 2, x));
  const boundedY = Math.min(rect.bottom - 2, Math.max(rect.top + 2, y));
  if (document.caretPositionFromPoint) {
    const p = document.caretPositionFromPoint(boundedX, boundedY);
    if (p) {
      const found = offset(item, p.offsetNode, p.offset);
      if (found != null) return found;
    }
  }
  const row = document.elementFromPoint(boundedX, boundedY)?.closest(".row[data-line]");
  return row && item.buffer.contains(row) ? item.lines[Number(row.dataset.line)].to : item.lines.at(-1).to;
}

function makePane() {
  const section = make("section", "pane"), buffer = make("div", "buffer"), mode = make("div", "mode");
  buffer.tabIndex = 0;
  buffer.setAttribute("role", "textbox");
  buffer.setAttribute("aria-multiline", "true");
  buffer.setAttribute("aria-readonly", "true");
  section.append(buffer, mode);
  get("panes").append(section);
  const item = {section, buffer, mode, current: null};
  buffer.addEventListener("click", (event) => {
    if (!snapshot || !item.current || event.detail > 1 || event.button !== 0 || (!event.shiftKey && !getSelection()?.isCollapsed)) return;
    const {view, revision} = item.current;
    send({type: "click", view, revision, pos: hit(item, event.clientX, event.clientY), extend: event.shiftKey});
    keys.focus({preventScroll: true});
  });
  buffer.addEventListener("scroll", () => {
    const top = buffer.scrollTop;
    if (!snapshot || !item.current || Math.abs(top - item.programmaticTop) < 1) return;
    clearTimeout(item.scrollTimer);
    const {view, revision, scroll} = item.current;
    const generation = snapshot.generation;
    item.scrollTimer = setTimeout(() => {
      const p = item.current;
      if (snapshot?.generation !== generation || !p || p.revision !== revision || p.scroll !== scroll) return;
      const anchor = visualAnchor(item);
      if (p.scroll !== anchor) send({type: "scroll", view, revision, anchor, caret: null});
    }, 45);
    positionCompletion();
    positionInput();
  }, {passive: true});
  return item;
}

function topLine(item) {
  let lo = 0, hi = item.rows.length;
  const top = item.buffer.scrollTop;
  while (lo + 1 < hi) {
    const mid = (lo + hi) >>> 1;
    if (item.rows[mid].offsetTop - item.rows[0].offsetTop <= top + 1) lo = mid;
    else hi = mid;
  }
  return lo;
}
function setScroll(item, y) {
  item.buffer.scrollTop = y;
  item.programmaticTop = item.buffer.scrollTop;
}

// The same logical line may occupy many visible rows. Take the browser's
// shaped line boxes as geometry, but send UTF-8 source locations to Techne.
function byteRect(item, byte) {
  const row = item.rows[lineAt(item.lines, byte)];
  for (const span of row.querySelectorAll(".text")) {
    const node = span.firstChild;
    const at = byte - Number(span.dataset.at);
    if (!node || at < 0 || at >= bytes(node.data)) continue;
    const utf16 = decoder.decode(encoder.encode(node.data).subarray(0, at)).length;
    const range = document.createRange();
    range.setStart(node, utf16);
    range.setEnd(node, utf16 + (node.data.codePointAt(utf16) > 0xffff ? 2 : 1));
    return range.getBoundingClientRect();
  }
  const tail = row.querySelector(".source")?.lastChild;
  return tail?.getClientRects?.()[tail.getClientRects().length - 1] || row.getBoundingClientRect();
}
function visualAnchor(item) {
  const bounds = item.buffer.getBoundingClientRect();
  const row = item.rows[topLine(item)];
  const source = row.querySelector(".source").getBoundingClientRect();
  // Sample just inside the first visible visual line, after the gutter.
  const y = Math.max(bounds.top, source.top) + 2;
  return hit(item, source.left + .5, y);
}
function scrollToByte(item, byte) {
  const row = item.rows[lineAt(item.lines, byte)];
  const source = row.querySelector(".source").getBoundingClientRect();
  const lineHeight = parseFloat(getComputedStyle(item.buffer).lineHeight) || 20;
  const rect = byteRect(item, byte);
  // A font's ink box may start a few pixels below the line box.
  const visual = Math.max(0, Math.floor((rect.top - source.top + 1) / lineHeight));
  setScroll(item, row.offsetTop - item.rows[0].offsetTop + visual * lineHeight);
}
function positionInput() {
  const item = panes.get(snapshot?.panes[snapshot.focus]?.view);
  const point = snapshot?.minibuffer ? get("command").querySelector(".caret") : item?.buffer.querySelector(".primary-caret");
  if (!point) return;
  const r = point.getBoundingClientRect();
  keys.style.left = `${Math.max(0, r.left)}px`;
  keys.style.top = `${Math.min(frame.clientHeight - 1, Math.max(0, r.top))}px`;
}
function pageRequest(item) {
  const p = item.current;
  if (!p.request) { item.lastRequest = null; return; }
  if (item.lastRequest === snapshot.id) return;
  item.lastRequest = snapshot.id;
  const head = p.selections[p.primary]?.[1] || 0;
  const point = byteRect(item, head);
  const buffer = item.buffer;
  const bounds = buffer.getBoundingClientRect();
  const rowHeight = parseFloat(getComputedStyle(buffer).lineHeight) || 20;
  const capacity = Math.max(1, Math.floor(buffer.clientHeight / rowHeight));
  const old = buffer.scrollTop;
  let target = null;
  if (p.request.Page) {
    const {screens, context} = p.request.Page;
    const count = Math.max(1, Math.abs(Math.round((Math.abs(screens) >= 1 ? Math.max(1, capacity - context) : capacity) * screens)));
    const end = screens > 0;
    const last = byteRect(item, item.lines.at(-1).to);
    if ((!end && old < 1) || (end && last.bottom <= bounds.bottom + 1)) {
      send({type: "edge", view: p.view, end});
      return;
    }
    setScroll(item, Math.min(old + (end ? count : -count) * rowHeight, old + last.top - bounds.top));
    target = hit(item, point.left + .5, point.top + point.height / 2);
  } else {
    const at = p.request.Recenter;
    const distance = at === "Middle" ? Math.floor(capacity / 2) : at === "Bottom" ? capacity - 1 : 0;
    const row = item.rows[lineAt(item.lines, head)].querySelector(".source").getBoundingClientRect();
    const visual = Math.max(0, Math.floor((point.top - row.top + 1) / rowHeight));
    setScroll(item, old + row.top - bounds.top + (visual - distance) * rowHeight);
  }
  const anchor = visualAnchor(item);
  clearTimeout(item.scrollTimer);
  send({type: "scroll", view: p.view, revision: p.revision, anchor, caret: target});
}

function drawRows(item, p, active, base) {
  const buffer = item.buffer;
  const selection = p.selections.map(([a, b]) => ({start: Math.min(a, b), end: Math.max(a, b), head: b}));
  const heads = new Set(selection.map((s) => s.head));
  const current = lineAt(item.lines, selection[p.primary]?.head || 0);
  const count = item.lines.length;
  const gutter = p.display.line_numbers !== "Off";
  const wrapping = `source${p.display.truncate_lines ? " truncate" : p.display.word_wrap ? " words" : ""}`;
  const width = Math.max(3, String(count).length) + 2;
  const output = document.createDocumentFragment();
  const rows = [];
  let layer = 0, hint = 0;
  for (let n = 0; n < count; n++) {
    const line = item.lines[n];
    const row = make("div", "row"), source = make("div", wrapping);
    row.dataset.line = n;
    if (gutter) {
      const label = make("span", "gutter");
      label.style.width = `${width}ch`;
      label.textContent = p.display.line_numbers === "Relative" && n !== current ? Math.abs(n - current) : n + 1;
      paint(label, chrome(n === current ? "line_number_current" : "line_number"));
      label.setAttribute("aria-hidden", "true");
      row.append(label);
    }
    while (layer < p.layers.length && p.layers[layer].to <= line.from) layer++;
    const bounds = new Set([line.from, line.to]);
    let endFace;
    for (let i = layer; i < p.layers.length && p.layers[i].from <= line.to; i++) {
      const h = p.layers[i];
      bounds.add(Math.max(h.from, line.from));
      bounds.add(Math.min(h.to, line.to));
      if (h.to > line.to && face(h.style).extend) endFace = face(h.style);
    }
    if (endFace) paint(source, endFace);
    for (const s of selection) if (s.end >= line.from && s.start <= line.to) {
      bounds.add(Math.max(s.start, line.from));
      bounds.add(Math.min(s.end, line.to));
      if (s.head >= line.from && s.head <= line.to) bounds.add(s.head);
    }
    for (let i = hint; i < p.inlays.length && p.inlays[i].at <= line.to; i++) {
      if (p.inlays[i].at >= line.from) bounds.add(p.inlays[i].at);
    }
    const stops = [...bounds].sort((a, b) => a - b);
    let h = layer;
    for (let i = 0; i < stops.length; i++) {
      const at = stops[i];
      const point = heads.has(at) && caret(active, p.cursor);
      if (point) {
        if (at === selection[p.primary]?.head) point.classList.add("primary-caret");
        source.append(point);
      }
      while (p.inlays[hint]?.at <= at) {
        const inlay = p.inlays[hint++];
        if (inlay.at < line.from) continue;
        const span = make("span", "inlay");
        span.dataset.at = at;
        span.textContent = inlay.text;
        paint(span, face(inlay.style));
        source.append(span);
      }
      const end = stops[i + 1];
      if (end == null) break;
      while (p.layers[h]?.to <= at) h++;
      const f = p.layers[h]?.from <= at ? face(p.layers[h].style) : {};
      const selected = selection.some((s) => s.start <= at && s.end > at);
      const span = make("span", "text");
      span.dataset.at = at;
      span.textContent = slice(line, at, end);
      paint(span, merge({...base, bg: null}, f, selected ? chrome("region") : {}), false);
      source.append(span);
    }
    row.append(source);
    rows.push(row);
    output.append(row);
  }
  if (p.display.eob_marker) {
    const padding = make("div", "eob");
    padding.setAttribute("aria-hidden", "true");
    paint(padding, chrome("eob_marker"));
    for (let i = 0; i < 75; i++) padding.append(Object.assign(make("div", ""), {textContent: p.display.eob_marker}));
    output.append(padding);
  }
  buffer.replaceChildren(output);
  item.rows = rows;
}

function renderPane(item, p, active, i) {
  const previous = item.current;
  const oldTop = item.buffer.scrollTop;
  const oldHead = previous?.selections[previous.primary]?.[1];
  const base = face(p.default), place = p.place;
  const s = item.section.style;
  [s.left, s.top, s.width, s.height] = [place.x, place.y, place.w, place.h].map((x) => `${x * 100}%`);
  s.borderColor = color(chrome("border").bg || chrome("border").fg);
  item.section.toggleAttribute("data-divider", place.x + place.w < .999);
  item.section.toggleAttribute("data-active", active);
  item.buffer.setAttribute("aria-label", active ? "Active buffer" : `Buffer pane ${i + 1}`);
  paint(item.buffer, {...base, height: null});
  paint(item.mode, merge({fg: base.fg}, chrome(active ? "mode_line" : "mode_line_inactive")));
  item.mode.textContent = p.status;
  if (!previous || previous.text !== p.text) item.lines = index(p.text);
  const signature = JSON.stringify([p.revision, p.selections, p.primary, p.cursor, p.display, p.layers, p.inlays, active, snapshot.styles]);
  if (item.signature !== signature || previous?.text !== p.text) {
    drawRows(item, p, active, base);
    item.signature = signature;
    item.buffer.scrollTop = oldTop;
    item.programmaticTop = item.buffer.scrollTop;
  }
  item.current = p;
  if (!previous || previous.scroll !== p.scroll) scrollToByte(item, p.scroll);
  const head = p.selections[p.primary]?.[1];
  if (active && (oldHead !== head || !previous)) {
    const wrap = item.buffer.getBoundingClientRect(), r = byteRect(item, head || 0);
    if (r.top < wrap.top || r.bottom > wrap.bottom) {
      setScroll(item, item.buffer.scrollTop + (r.top < wrap.top ? r.top - wrap.top : r.bottom - wrap.bottom));
    }
  }
}

function choices(element, rows, selected, kind) {
  const normal = kind === "completion" ? chrome("completion") : {};
  const choice = chrome(kind === "completion" ? "completion_selected" : "selected");
  const output = document.createDocumentFragment();
  for (const [i, row] of rows.entries()) {
    const item = make("div", "option");
    item.setAttribute("role", "option");
    item.setAttribute("aria-selected", String(selected === i));
    paint(item, merge(normal, selected === i ? choice : {}));
    for (const runs of row.columns) {
      const cell = make("span", "");
      for (const run of runs) {
        const span = make("span", "");
        span.textContent = run.text;
        paint(span, merge(normal, face(run.style), selected === i ? choice : {}));
        cell.append(span);
      }
      item.append(cell);
    }
    output.append(item);
  }
  element.replaceChildren(output);
  element.hidden = rows.length === 0;
}
function positionCompletion() {
  const c = snapshot?.completion, box = get("completion");
  if (!c || box.hidden) return;
  const item = panes.get(c.view);
  if (!item) { box.hidden = true; return; }
  const point = item.buffer.querySelector(".primary-caret");
  const r = point?.getBoundingClientRect() || item.buffer.getBoundingClientRect();
  const bounds = item.buffer.getBoundingClientRect();
  const below = bounds.bottom - r.bottom, above = r.top - bounds.top;
  if (r.top < bounds.top || r.top >= bounds.bottom) { box.style.visibility = "hidden"; return; }
  box.style.visibility = "visible";
  const width = box.offsetWidth, height = box.offsetHeight;
  box.style.left = `${Math.max(0, Math.min(r.left, frame.clientWidth - width))}px`;
  box.style.top = `${Math.max(bounds.top, below >= height || below >= above ? r.bottom : r.top - height)}px`;
}
function render(next) {
  snapshot = next;
  paint(frame, {...face(next.panes[0]?.default), height: null});
  const ids = new Set(next.panes.map((p) => p.view));
  for (const [id, item] of panes) if (!ids.has(id)) {
    item.section.remove();
    clearTimeout(item.scrollTimer);
    panes.delete(id);
  }
  const mini = next.minibuffer;
  get("minibuffer").hidden = !mini;
  if (mini) {
    paint(get("prompt-line"), mini.input_selected ? chrome("selected") : {});
    paint(get("prompt"), chrome("prompt"));
    get("prompt").textContent = mini.prompt;
    const input = encoder.encode(mini.input);
    get("command").replaceChildren(decoder.decode(input.subarray(0, mini.caret)), caret(true), decoder.decode(input.subarray(mini.caret)));
    choices(get("choices"), mini.rows, mini.selected, "mini");
  }
  const hints = get("hints");
  hints.hidden = !next.hints.length;
  hints.replaceChildren(...next.hints.map((hint) => {
    const row = make("span", "hint"), chord = make("span", ""), colon = make("span", ""), description = make("span", "");
    chord.textContent = hint.key;
    colon.textContent = " : ";
    description.textContent = hint.description;
    paint(chord, chrome("which_key_key"));
    paint(colon, chrome("which_key_separator"));
    if (hint.prefix) paint(description, chrome("which_key_group"));
    row.append(chord, colon, description);
    return row;
  }));
  get("echo").textContent = next.echo;
  next.panes.forEach((p, i) => {
    if (!panes.has(p.view)) panes.set(p.view, makePane());
    renderPane(panes.get(p.view), p, next.focus === i, i);
  });
  choices(get("completion"), next.completion?.rows || [], next.completion?.selected, "completion");
  positionCompletion();
  positionInput();
  next.panes.forEach((p) => pageRequest(panes.get(p.view)));
  viewport();
  status("Connected", true);
  if (document.activeElement === document.body || document.activeElement === get("reconnect")) keys.focus({preventScroll: true});
}

// Geometry in the editor's cells, as native frontends report it: Lisp
// lays out splits and fits terminal programs from it.
let reported = null;
function viewport() {
  if (!snapshot) return;
  const probe = make("span", "");
  probe.textContent = "0".repeat(64);
  probe.style.cssText = "position:absolute;visibility:hidden;white-space:pre";
  frame.append(probe);
  const cell = probe.getBoundingClientRect().width / 64 || 9;
  probe.remove();
  const line = parseFloat(getComputedStyle(frame).lineHeight) || 20;
  const columns = Math.max(1, Math.floor((frame.clientWidth - 12) / cell));
  const rows = Math.max(2, Math.floor(frame.clientHeight / line));
  // A divider and both panes' padding fall between split panes.
  const gap = Math.ceil(13 / cell) + 1;
  const next = `${snapshot.generation}:${columns}x${rows}+${gap}`;
  if (next !== reported && send({type: "viewport", columns, rows, split_gap: gap})) reported = next;
}

function invalidate() {
  snapshot = undefined;
  reported = null;
  clipboard = "";
  composing = false;
  keys.value = "";
  for (const item of panes.values()) {
    clearTimeout(item.scrollTimer);
    item.current = null;
    item.signature = null;
  }
}

function connect() {
  if (!token) { status("Open the private URL printed by techne web."); return; }
  socket?.close();
  invalidate();
  const ws = new WebSocket(`ws://${location.host}/attach`, ["techne-v3", token]);
  socket = ws;
  status("Connecting…");
  let reason = "Disconnected from Techne.";
  ws.addEventListener("message", ({data}) => {
    if (ws !== socket) return;
    const message = JSON.parse(data);
    switch (message.type) {
      case "snapshot": render(message); break;
      case "clipboard": if (snapshot) clipboard = message.text; break;
      case "error": invalidate(); reason = message.message; status(reason); break;
      case "restart": invalidate(); status("Editor restarted; waiting for its view…"); break;
      case "quit": invalidate(); reason = "Editor session closed."; status(reason); break;
    }
  });
  ws.addEventListener("close", () => { if (ws === socket) { invalidate(); status(reason); } });
  ws.addEventListener("error", () => { reason = "Connection failed. Check the private URL and gateway."; });
}
get("reconnect").addEventListener("click", connect);
get("minibuffer").addEventListener("click", () => keys.focus({preventScroll: true}));
window.addEventListener("resize", () => { positionCompletion(); positionInput(); viewport(); });
window.addEventListener("focus", () => send({type: "focus", focused: true}));
window.addEventListener("blur", () => send({type: "focus", focused: false}));
connect();
