/* Lurkly chat (v2.1): native Twitch chat renderer. Global `LurklyChat`. Contract: docs/v2/chat-contract.md
   All message text is inserted with textContent / createTextNode; no innerHTML is used with data. */
(function () {
"use strict";
const CAP = 400, CAP_PAUSED = 600, MAXEMO = 30, WARM = 3, FRAME = 60, QMAX = 900;
const TWURL = id => (O && O.twitchEmote) ? O.twitchEmote(id) : "https://static-cdn.jtvnw.net/emoticons/v2/" + encodeURIComponent(id) + "/default/dark/2.0";
const PROV = { "7tv": "7TV", bttv: "BTTV", ffz: "FFZ", twitch: "Twitch" };
const NS = "http://www.w3.org/2000/svg";

let O = null, root = null, X = {}, chans = new Map(), warm = [], active = null;
let gs = { scope: null, me: null, at: 0 }, hideTimer = 0, scheduled = false, picker = null;
let prefs = { timestamps: false, badges: true, density: "comfortable" };

const el = (t, c, txt) => { const e = document.createElement(t); if (c) e.className = c; if (txt != null) e.textContent = txt; return e; };
const svg = (d, cls) => { const s = document.createElementNS(NS, "svg"); s.setAttribute("viewBox", "0 0 16 16"); if (cls) s.setAttribute("class", cls); const p = document.createElementNS(NS, "path"); p.setAttribute("d", d); s.appendChild(p); return s; };
const ICO = { reply: "M6.5 3.5 2.5 7l4 3.5M2.5 7h7a4 4 0 0 1 4 4v1.5", copy: "M5.5 5.5h7v8h-7zM3.5 10.5v-8h7", x: "M4 4l8 8M12 4l-8 8", smile: "M8 14a6 6 0 1 0 0-12 6 6 0 0 0 0 12zM5.5 9.5a3 3 0 0 0 5 0M6 6.5v.5M10 6.5v.5", send: "M2.5 8 13.5 2.5 10 13.5 8 9z", down: "M8 3v9M4 8l4 4 4-4", search: "M7 11.5a4.5 4.5 0 1 0 0-9 4.5 4.5 0 0 0 0 9zM10.5 10.5l3 3" };
const call = (cmd, args) => { try { return Promise.resolve(O.inv(cmd, args)); } catch (e) { return Promise.reject(e); } };
const lsGet = k => { try { return localStorage.getItem("lurkly." + k); } catch (e) { return null; } };
const lsSet = (k, v) => { try { localStorage.setItem("lurkly." + k, v); } catch (e) {} };

/* ---------- colour ---------- */
const hex2rgb = h => { h = h.replace("#", ""); return [0, 2, 4].map(i => parseInt(h.substr(i, 2), 16)); };
const rgb2hex = c => "#" + c.map(v => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, "0")).join("");
const lumin = c => { const f = v => { v /= 255; return v <= .03928 ? v / 12.92 : Math.pow((v + .055) / 1.055, 2.4); }; return .2126 * f(c[0]) + .7152 * f(c[1]) + .0722 * f(c[2]); };
const ratio = (a, b) => { const l1 = lumin(a), l2 = lumin(b); return (Math.max(l1, l2) + .05) / (Math.min(l1, l2) + .05); };
function rgb2hsl(c) { const r = c[0] / 255, g = c[1] / 255, b = c[2] / 255, mx = Math.max(r, g, b), mn = Math.min(r, g, b), l = (mx + mn) / 2; let h = 0, s = 0;
  if (mx !== mn) { const d = mx - mn; s = l > .5 ? d / (2 - mx - mn) : d / (mx + mn); h = mx === r ? (g - b) / d + (g < b ? 6 : 0) : mx === g ? (b - r) / d + 2 : (r - g) / d + 4; h /= 6; } return [h, s, l]; }
function hsl2rgb(h, s, l) { if (!s) return [l * 255, l * 255, l * 255]; const q = l < .5 ? l * (1 + s) : l + s - l * s, p = 2 * l - q;
  const f = t => { if (t < 0) t += 1; if (t > 1) t -= 1; return t < 1 / 6 ? p + (q - p) * 6 * t : t < .5 ? q : t < 2 / 3 ? p + (q - p) * (2 / 3 - t) * 6 : p; };
  return [f(h + 1 / 3) * 255, f(h) * 255, f(h - 1 / 3) * 255]; }
const colCache = new Map();
function bgRGB() { const v = root ? getComputedStyle(root).getPropertyValue("--bg").trim() : ""; if (/^#[0-9a-f]{6}$/i.test(v)) return hex2rgb(v); return isDark() ? [20, 19, 24] : [244, 243, 247]; }
const isDark = () => { try { return O.theme() === "dark"; } catch (e) { return true; } };
function fixColor(hex) {
  const key = (isDark() ? "d" : "l") + hex, hit = colCache.get(key); if (hit) return hit;
  const bg = bgRGB(); let rgb = hex2rgb(hex);
  if (ratio(rgb, bg) < 4.5) { const [h, s, l0] = rgb2hsl(rgb), dir = lumin(bg) < .5 ? 1 : -1; let l = l0;
    for (let i = 0; i < 40; i++) { l += dir * .025; if (l > 1 || l < 0) { l = Math.max(0, Math.min(1, l)); rgb = hsl2rgb(h, s, l); break; } rgb = hsl2rgb(h, s, l); if (ratio(rgb, bg) >= 4.5) break; } }
  const out = rgb2hex(rgb); colCache.set(key, out); return out;
}
function nameColor(it) {
  if (/^#[0-9a-f]{6}$/i.test(it.color || "")) return it.color;
  let h = 0; const s = it.login || it.name || ""; for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
  return rgb2hex(hsl2rgb((h % 15) / 15, .7, .6));
}

/* ---------- channel buffers ---------- */
function mkChan(login) {
  const list = el("div", "lc-list off"); list.setAttribute("role", "log"); list.setAttribute("aria-live", "off"); list.setAttribute("aria-label", "Chat messages"); list.tabIndex = 0;
  const ch = { login, list, q: [], byId: new Map(), paused: false, newCount: 0, state: "connecting", msg: "", room: null, em: null, map: new Map(), emReady: false, emErr: false,
    t0: performance.now(), joined: false, draft: "", reply: null, chatters: new Map(), st: 0, last: -9, stick: false, cool: 0 };
  list.addEventListener("scroll", () => { if (Math.abs(list.scrollTop - ch.last) < 2) return; const stuck = list.scrollHeight - list.scrollTop - list.clientHeight < 40; if (stuck) { ch.paused = false; ch.newCount = 0; } else ch.paused = true; if (ch.login === active) updatePill(); }, { passive: true });
  const stick = () => { if (ch.stick || ch.paused) return; ch.stick = true; requestAnimationFrame(() => { ch.stick = false; if (!ch.paused) toBottom(ch); }); };
  list.addEventListener("load", stick, true); list.addEventListener("error", stick, true);
  list.addEventListener("click", e => onListClick(ch, e));
  list.addEventListener("keydown", e => onListKey(ch, e));
  X.main.appendChild(list); chans.set(login, ch); return ch;
}
function toBottom(ch) { ch.list.scrollTop = ch.list.scrollHeight; ch.last = ch.list.scrollTop; }
function join(ch) {
  ch.joined = true; ch.t0 = performance.now();
  call("chat_join", { login: ch.login }).catch(e => setState(ch, "error", errText(e)));
  loadEmotes(ch);
}
const errText = e => (e && (e.message || e)) ? String(e.message || e) : "Can't reach chat.";
function loadEmotes(ch) {
  ch.emErr = false; ch.emReady = false;
  call("chat_emotes", { login: ch.login }).then(em => {
    ch.em = em || {}; ch.map = new Map();
    for (const t of (em && em.third) || []) if (t && t.n && !ch.map.has(t.n)) ch.map.set(t.n, t);
    ch.emReady = true; ch.pk = null; if (active === ch.login) { updateBanner(); if (picker && picker.isOpen()) picker.build(); } schedule();
  }).catch(() => { ch.emReady = true; ch.emErr = true; ch.pk = null; if (active === ch.login) updateBanner(); schedule(); });
}
function leave(login) {
  const ch = chans.get(login); if (!ch) return;
  if (ch.joined) call("chat_leave", { login }).catch(() => {});
  ch.list.remove(); chans.delete(login); warm = warm.filter(l => l !== login);
}

/* ---------- message building ---------- */
function newBody(ch, me) { return { ch, me, body: el("span", "body"), buf: "", last: null, n: 0, plain: "" }; }
function flushBuf(s) { if (s.buf) { s.body.appendChild(document.createTextNode(s.buf)); s.buf = ""; } }
function addEmote(s, url, name, prov, zw) {
  if (s.n >= MAXEMO) { s.buf += name; s.plain += name; s.last = null; return; }
  s.n++; s.plain += name; flushBuf(s);
  const img = el("img", "emo"); img.decoding = "async"; img.draggable = false; img.alt = name; img.src = url;
  if (zw && s.last) {
    img.className = "zw"; img.alt = ""; img.setAttribute("aria-hidden", "true"); img.onerror = () => img.remove();
    let host = s.last; if (!host.classList.contains("stack")) { const w = el("span", "stack"); s.body.replaceChild(w, host); w.appendChild(host); s.last = w; host = w; }
    host.title = (host.title ? host.title + " + " : "") + name + " · " + prov; host.appendChild(img); return;
  }
  img.title = name + " · " + prov; img.onerror = () => img.replaceWith(document.createTextNode(name));
  s.body.appendChild(img); s.last = img;
}
function addText(s, str) {
  s.plain += str;
  for (const part of str.split(/(\s+)/)) {
    if (!part) continue;
    if (/^\s+$/.test(part)) { s.buf += part; continue; }
    const t = s.ch.map.get(part);
    if (t) addEmote(s, t.u, t.n, PROV[t.src] || t.src || "", !!t.zw);
    else { s.buf += part; s.last = null; }
  }
}
function buildBody(ch, it, me) {
  const s = newBody(ch, me);
  for (const f of it.frags || []) {
    if (f.k === "e") addEmote(s, TWURL(f.id), f.v, "Twitch", false);
    else if (f.k === "m") { flushBuf(s); const a = el("span", "at", f.v); const who = f.v.replace(/^@/, "").toLowerCase();
      if (me && (who === (me.login || "").toLowerCase() || who === (me.name || "").toLowerCase())) a.classList.add("me"); s.body.appendChild(a); s.plain += f.v; s.last = null; }
    else if (f.k === "c") { s.buf += f.v; s.plain += f.v; s.last = null; }
    else addText(s, String(f.v == null ? "" : f.v));
  }
  flushBuf(s); return s;
}
const BRANK = { broadcaster: 0, moderator: 1, vip: 2, subscriber: 3 };
const BLET = { broadcaster: ["bc", "B"], moderator: ["mod", "M"], vip: ["vip", "V"], subscriber: ["sub", "S"] };
function addBadges(row, ch, badges) {
  const list = (badges || []).slice().sort((a, b) => (BRANK[a.s] ?? 9) - (BRANK[b.s] ?? 9)), map = (ch.em && ch.em.badges) || {};
  for (const b of list) {
    const url = map[b.s + "/" + b.v] || map[b.s + "/1"];
    if (url) { const i = el("img", "bimg"); i.src = url; i.alt = b.s; i.title = b.s + (b.v && b.v !== "1" ? " " + b.v : ""); i.draggable = false; i.onerror = () => i.remove(); row.appendChild(i); }
    else if (BLET[b.s]) { const s = el("span", "bg " + BLET[b.s][0], BLET[b.s][1]); s.title = b.s; s.setAttribute("aria-hidden", "true"); row.appendChild(s); }
  }
}
const hhmm = ts => { const d = new Date(ts || Date.now()); return String(d.getHours()).padStart(2, "0") + ":" + String(d.getMinutes()).padStart(2, "0"); };
function buildMsg(ch, it) {
  const me = gs.me, row = el("div", "m"); row.tabIndex = -1;
  if (it.id) { row.dataset.id = it.id; ch.byId.set(it.id, row); }
  row.dataset.uid = it.uid || ""; row._login = it.login; row._name = it.name || it.login;
  if (it.login) ch.chatters.set((it.name || it.login), Date.now());
  if (ch.chatters.size > 300) { const k = ch.chatters.keys().next().value; ch.chatters.delete(k); }
  const meL = me ? [(me.login || "").toLowerCase(), (me.name || "").toLowerCase()] : [];
  if (it.hl || (it.reply && it.reply.login && meL.includes(String(it.reply.login).toLowerCase()) && !it.self)) row.classList.add("mention");
  if (it.self) row.classList.add("self"); if (it.action) row.classList.add("action");
  if (it.reply) {
    const r = el("div", "rep"); r.dataset.pid = it.reply.id || ""; r.appendChild(svg(ICO.reply));
    r.appendChild(el("span", "", "Replying to @" + (it.reply.name || it.reply.login) + ": " + (it.reply.text || ""))); row.appendChild(r);
  }
  const ts = el("span", "ts", hhmm(it.ts)); row.appendChild(ts);
  if (it.first) { const f = el("span", "fm", "first"); f.title = "First message in this channel"; row.appendChild(f); }
  addBadges(row, ch, it.badges);
  const c = nameColor(it); row._c = c;
  const u = el("span", "u", it.name || it.login); u.style.color = fixColor(c); u.title = "Mention " + (it.name || it.login); row.appendChild(u);
  if (!it.action) row.appendChild(el("span", "sep2", ":")); else row.appendChild(document.createTextNode(" "));
  const s = buildBody(ch, it, me); if (it.action) s.body.style.color = fixColor(c);
  row._t = s.plain; row.appendChild(s.body);
  const dm = el("span", "dm", "<message deleted>"), pk = el("button", "peek", "show"); pk.type = "button"; row.appendChild(dm); row.appendChild(pk);
  const acts = el("div", "acts"); const rb = el("button"), cb = el("button"); rb.type = cb.type = "button"; rb.dataset.a = "reply"; cb.dataset.a = "copy";
  rb.title = rb.ariaLabel = "Reply"; cb.title = cb.ariaLabel = "Copy message"; rb.appendChild(svg(ICO.reply)); cb.appendChild(svg(ICO.copy)); acts.appendChild(rb); acts.appendChild(cb); row.appendChild(acts);
  return row;
}
function buildSys(ch, it, cls, text) { const r = el("div", "m sys " + (cls || "")); r.textContent = text; r.setAttribute("role", "note"); return r; }
function markDeleted(row) { if (!row || row.classList.contains("del")) return; row.classList.add("del"); const pk = row.querySelector(".peek"); if (pk) pk.setAttribute("aria-expanded", "false"); }

/* ---------- queue + flush ---------- */
function onItem(it) {
  if (!it || !it.ch) return; const ch = chans.get(String(it.ch).toLowerCase()); if (!ch) return;
  if (it.t === "status") { setState(ch, it.state, it.msg || ""); return; }
  if (it.t === "room") { ch.room = it; if (active === ch.login) updateRoom(); return; }
  ch.q.push(it); if (ch.q.length > QMAX) ch.q.splice(0, ch.q.length - QMAX + 200);
}
function schedule() { if (scheduled) return; scheduled = true; requestAnimationFrame(flush); }
function flush() {
  scheduled = false; if (document.hidden || !active) return;
  const ch = chans.get(active); if (!ch || !ch.q.length) return;
  if (!ch.emReady && performance.now() - ch.t0 < 2000) { setTimeout(schedule, 120); return; }
  if (ch.q.length > 300 && !ch.paused) ch.q.splice(0, ch.q.length - 300);
  const batch = ch.q.splice(0, FRAME), frag = document.createDocumentFragment(), list = ch.list; let added = 0;
  const before = list.scrollHeight;
  for (const it of batch) {
    if (it.t === "msg") { frag.appendChild(buildMsg(ch, it)); added++; }
    else if (it.t === "del") markDeleted(ch.byId.get(it.id));
    else if (it.t === "clear") {
      if (!it.uid) { for (const r of list.children) markDeleted(r); for (const r of frag.children) markDeleted(r); frag.appendChild(buildSys(ch, it, "info", "Chat was cleared by a moderator")); }
      else { let who = ""; const hit = r => { if (r.dataset && r.dataset.uid === String(it.uid)) { who = who || r._name; markDeleted(r); } };
        for (const r of list.children) hit(r); for (const r of frag.children) hit(r); if (who) frag.appendChild(buildSys(ch, it, "info", who + " was timed out or banned")); }
    }
    else if (it.t === "notice") frag.appendChild(buildSys(ch, it, it.kind, it.text || ""));
  }
  if (!frag.firstChild && !batch.length) return;
  list.appendChild(frag);
  const cap = ch.paused ? CAP_PAUSED : CAP; let excess = list.childElementCount - cap;
  if (excess > 0) {
    const h0 = list.scrollHeight;
    while (excess-- > 0) { const r = list.firstElementChild; if (r.dataset.id && ch.byId.get(r.dataset.id) === r) ch.byId.delete(r.dataset.id); r.remove(); }
    if (ch.paused) list.scrollTop = Math.max(0, list.scrollTop - (h0 - list.scrollHeight));
  }
  if (ch.paused) ch.newCount += added; else toBottom(ch);
  if (ch.newCount && !ch.paused) ch.newCount = 0;
  X.root.dataset.empty = list.childElementCount ? "0" : "1"; updatePill();
  if (ch.q.length) schedule();
}

/* ---------- status / ui state ---------- */
function setState(ch, state, msg) {
  ch.state = state; ch.msg = msg || ""; if (state === "live") ch.msg = "";
  try { O.onState && O.onState(ch.login, state, ch.msg); } catch (e) {}
  if (ch.login === active) renderState();
}
function renderState() {
  const ch = chans.get(active); if (!ch) return;
  X.root.dataset.state = ch.state; X.root.dataset.empty = ch.list.childElementCount ? "0" : "1";
  updateBanner(); updateRoom(); updateCompose(); updatePill();
}
function updateBanner() {
  const ch = chans.get(active), b = X.banner; if (!ch) return; b.replaceChildren(); let cls = "", txt = "", btn = null, fn = null;
  if (ch.state === "reconnecting") { cls = "warn"; txt = ch.msg || "Connection lost. Reconnecting..."; }
  else if (ch.state === "error") { cls = "err"; txt = (ch.msg || "Can't reach chat.") + " History stays."; btn = "Retry"; fn = () => { setState(ch, "connecting"); call("chat_join", { login: ch.login }).catch(e => setState(ch, "error", errText(e))); }; }
  else if (ch.emErr) { cls = "warn"; txt = "Emotes unavailable. Plain text shown; chat still works."; btn = "Retry"; fn = () => { loadEmotes(ch); updateBanner(); }; }
  b.className = "lc-banner" + (cls ? " on " + cls : ""); if (!cls) return;
  b.appendChild(el("span", "", txt)); if (btn) { const bt = el("button", "", btn); bt.type = "button"; bt.onclick = fn; b.appendChild(bt); }
}
function updateRoom() {
  const ch = chans.get(active), r = X.room; r.replaceChildren(); const rm = ch && ch.room; if (!rm) { r.className = "lc-room"; return; }
  const add = t => r.appendChild(el("span", "", t));
  if (rm.slow > 0) add("Slow " + rm.slow + " s"); if (rm.subs) add("Subs only"); if (rm.emote_only) add("Emote only");
  if (rm.followers >= 0 && rm.followers != null) add(rm.followers === 0 ? "Followers only" : "Followers " + (rm.followers >= 60 ? Math.round(rm.followers / 60) + " h" : rm.followers + " m"));
  r.className = "lc-room" + (r.childNodes.length ? " on" : "");
}
function updatePill() {
  const ch = chans.get(active), p = X.pill; if (!ch) { p.classList.remove("on"); return; }
  p.classList.toggle("on", !!ch.paused);
  X.pillT.textContent = ch.newCount ? "Chat paused, " + ch.newCount + " new message" + (ch.newCount === 1 ? "" : "s") : "Chat paused";
}
function resume(ch) { ch.paused = false; ch.newCount = 0; toBottom(ch); updatePill(); schedule(); }
function noScope() { const ch = chans.get(active); return gs.scope === false || (ch && ch.state === "no-scope"); }
function updateCompose() {
  const ch = chans.get(active); if (!ch) return; const inp = X.input, st = ch.state, ns = noScope();
  X.root.classList.toggle("signedout", ns);
  let dis = false, ph = "Send a message";
  if (st === "connecting") { dis = true; ph = "Connecting..."; }
  else if (st === "reconnecting") { dis = true; ph = "Reconnecting..."; }
  else if (st === "error") { dis = true; ph = "Can't reach chat"; }
  else if (!gs.me) { dis = true; ph = "Sign in to chat"; }
  if (ch.cool > Date.now()) { dis = true; ph = "Slow mode: " + Math.ceil((ch.cool - Date.now()) / 1000) + " s"; }
  inp.disabled = dis; inp.placeholder = ph; X.inpWrap.classList.toggle("dis", dis);
  X.send.disabled = dis || !inp.value.trim(); X.emb.disabled = dis;
  inp.setAttribute("aria-label", dis ? ph : "Send a message to " + ch.login);
  X.signinT.textContent = gs.scope === false || st === "no-scope" ? "Sending needs one more permission." : "";
  renderReply(ch);
}
function renderReply(ch) {
  const r = ch.reply, c = X.reply; c.classList.toggle("on", !!r); if (!r) return; c.replaceChildren();
  const s = el("span"); s.appendChild(document.createTextNode("Replying to ")); s.appendChild(el("b", "", "@" + r.name)); s.appendChild(document.createTextNode(": " + r.text)); c.appendChild(s);
  const x = el("button"); x.type = "button"; x.title = x.ariaLabel = "Cancel reply (Esc)"; x.appendChild(svg(ICO.x)); x.onclick = () => { ch.reply = null; renderReply(ch); X.input.focus(); }; c.appendChild(x);
}
function showErr(msg) { const e = X.err; e.textContent = msg || ""; e.classList.toggle("on", !!msg); clearTimeout(showErr.t); if (msg) showErr.t = setTimeout(() => showErr(""), 7000); }

/* ---------- list interactions ---------- */
function setReply(ch, row) {
  if (!row || !row.dataset.id || row.classList.contains("sys")) return;
  ch.reply = { id: row.dataset.id, login: row._login, name: row._name, text: (row._t || "").slice(0, 120) }; renderReply(ch); X.input.focus();
}
function insertAtCaret(text) {
  const i = X.input, s = i.selectionStart == null ? i.value.length : i.selectionStart, e = i.selectionEnd == null ? s : i.selectionEnd, v = i.value;
  const pre = v.slice(0, s), sp = pre && !/\s$/.test(pre) ? " " : "";
  i.value = pre + sp + text + v.slice(e); const pos = (pre + sp + text).length; i.setSelectionRange(pos, pos); i.focus(); onInput();
}
function onListClick(ch, e) {
  const row = e.target.closest(".m"); if (!row) return;
  const btn = e.target.closest("button");
  if (btn && btn.dataset.a === "reply") { setReply(ch, row); return; }
  if (btn && btn.dataset.a === "copy") { try { navigator.clipboard.writeText(row._t || "").then(() => O.toast && O.toast("Copied"), () => {}); } catch (x) {} return; }
  if (e.target.closest(".peek")) { const on = row.classList.toggle("shown"); e.target.setAttribute("aria-expanded", on); e.target.textContent = on ? "hide" : "show"; return; }
  if (e.target.closest(".u") && !row.classList.contains("sys")) { if (gs.me && !X.input.disabled) insertAtCaret("@" + row._name + " "); return; }
  const rep = e.target.closest(".rep");
  if (rep) { const p = ch.byId.get(rep.dataset.pid); if (p) { p.scrollIntoView({ block: "center" }); p.classList.remove("flash"); void p.offsetWidth; p.classList.add("flash"); } }
}
function onListKey(ch, e) {
  const k = e.key, rows = ch.list.children;
  if (k === "End") { e.preventDefault(); resume(ch); return; }
  if (k === "ArrowUp" || k === "ArrowDown") {
    const cur = document.activeElement && document.activeElement.closest && document.activeElement.closest(".m"); let t;
    if (!cur || cur.parentNode !== ch.list) t = ch.list.lastElementChild; else t = k === "ArrowUp" ? cur.previousElementSibling : cur.nextElementSibling;
    if (t) { e.preventDefault(); t.focus(); } return;
  }
  if ((k === "r" || k === "R") && !e.ctrlKey && !e.metaKey && !e.altKey) { const row = document.activeElement.closest && document.activeElement.closest(".m"); if (row && row.parentNode === ch.list) { e.preventDefault(); setReply(ch, row); } }
}

/* ---------- send box ---------- */
function onInput() {
  const ch = chans.get(active), v = X.input.value, n = [...v].length;
  X.counter.textContent = n + "/500"; X.counter.classList.toggle("on", n >= 400); X.counter.classList.toggle("over", n > 500);
  X.send.disabled = X.input.disabled || !v.trim() || n > 500; if (ch) ch.draft = v;
  showErr(""); acUpdate();
}
async function doSend() {
  const ch = chans.get(active); if (!ch || X.input.disabled) return; const text = X.input.value.trim(); if (!text) return;
  if ([...text].length > 500) { showErr("Message is over 500 characters."); return; }
  const rep = ch.reply; X.input.value = ""; ch.draft = ""; ch.reply = null; onInput(); renderReply(ch); resume(ch); acClose();
  X.send.disabled = true;
  try {
    await call("chat_send", { login: ch.login, text, replyTo: rep ? rep.id : null });
    if (ch.room && ch.room.slow > 0) { ch.cool = Date.now() + ch.room.slow * 1000; cooldownTick(ch); }
  } catch (e) {
    if (!X.input.value && chans.get(active) === ch) { X.input.value = text; ch.reply = rep; renderReply(ch); onInput(); }
    showErr(errText(e));
  }
  updateCompose();
}
function cooldownTick(ch) {
  if (active === ch.login) updateCompose();
  if (ch.cool > Date.now()) setTimeout(() => cooldownTick(ch), 500);
  else if (active === ch.login) { updateCompose(); if (document.activeElement === document.body) X.input.focus(); }
}

/* ---------- autocomplete ---------- */
const ac = { items: [], sel: 0, start: 0, end: 0, on: false };
function acSources(ch) { const out = []; const em = ch.em || {}; for (const t of em.twitch || []) out.push({ n: t.n, u: TWURL(t.id), p: "Twitch" }); for (const t of em.third || []) out.push({ n: t.n, u: t.u, p: PROV[t.src] || t.src, zw: t.zw }); return out; }
function acUpdate() {
  const ch = chans.get(active), i = X.input; if (!ch || i.disabled) return acClose();
  const pos = i.selectionStart == null ? i.value.length : i.selectionStart, pre = i.value.slice(0, pos), m = /(?:^|\s)([:@])([^\s:@]*)$/.exec(pre);
  if (!m) return acClose(); const kind = m[1], q = m[2].toLowerCase();
  if ((kind === ":" && q.length < 2) || (kind === "@" && q.length < 1)) return acClose();
  let items = [];
  if (kind === ":") { const pre1 = [], rest = []; for (const s of acSources(ch)) { const n = s.n.toLowerCase(); if (n.startsWith(q)) pre1.push(s); else if (n.includes(q)) rest.push(s); if (pre1.length >= 6) break; } items = pre1.concat(rest).slice(0, 6); }
  else { const names = [...ch.chatters.keys()].reverse().filter(n => n.toLowerCase().startsWith(q)); items = names.slice(0, 6).map(n => ({ n, p: "chatter", user: true })); }
  if (!items.length) return acClose();
  ac.items = items; ac.sel = 0; ac.end = pos; ac.start = pos - m[2].length - 1; ac.on = true; acRender();
}
function acRender() {
  const box = X.ac; box.replaceChildren();
  ac.items.forEach((s, k) => {
    const b = el("button", "lc-aci" + (k === ac.sel ? " sel" : "")); b.type = "button"; b.id = "lc-ac" + k; b.setAttribute("role", "option"); b.setAttribute("aria-selected", k === ac.sel); b.tabIndex = -1;
    const pv = el("span", "pv"); if (s.u) { const im = el("img"); im.src = s.u; im.alt = ""; im.style.height = "var(--emo)"; pv.appendChild(im); } b.appendChild(pv);
    b.appendChild(el("span", "nm", (s.user ? "@" : "") + s.n)); b.appendChild(el("span", "tg", s.p));
    b.onmousedown = e => { e.preventDefault(); acAccept(k); }; box.appendChild(b);
  });
  box.classList.add("on"); X.input.setAttribute("aria-activedescendant", "lc-ac" + ac.sel); X.input.setAttribute("aria-expanded", "true");
}
function acClose() { if (!ac.on) return; ac.on = false; X.ac.classList.remove("on"); X.ac.replaceChildren(); X.input.removeAttribute("aria-activedescendant"); X.input.setAttribute("aria-expanded", "false"); }
function acAccept(k) {
  const s = ac.items[k == null ? ac.sel : k]; if (!s) return; const i = X.input, v = i.value, ins = (s.user ? "@" : "") + s.n + " ";
  i.value = v.slice(0, ac.start) + ins + v.slice(ac.end); const p = ac.start + ins.length; i.setSelectionRange(p, p); acClose(); onInput(); i.focus();
}
function acMove(d) { ac.sel = (ac.sel + d + ac.items.length) % ac.items.length; acRender(); }

/* ---------- emote picker ---------- */
function makePicker() {
  const P = X.picker; let open = false, timer = 0, lastFocus = null;
  const body = X.pbody, search = X.psearch, foot = X.pfoot, jump = X.pjump;
  const recent = () => { try { return JSON.parse(lsGet("chat.recent") || "[]"); } catch (e) { return []; } };
  const pushRecent = e => { const r = recent().filter(x => x.n !== e.n); r.unshift({ n: e.n, u: e.u, p: e.p, zw: e.zw }); lsSet("chat.recent", JSON.stringify(r.slice(0, 8))); };
  function sections() {
    const ch = chans.get(active), em = (ch && ch.em) || {}; if (ch && ch.pk) return ch.pk;
    const tw = (em.twitch || []).slice().sort((a, b) => (a.scope === b.scope ? 0 : a.scope === "channel" ? -1 : 1)).map(t => ({ n: t.n, u: TWURL(t.id), p: "Twitch" }));
    const th = src => (em.third || []).filter(t => t.src === src).map(t => ({ n: t.n, u: t.u, p: PROV[src], zw: t.zw, scope: t.scope }));
    const out = [["Twitch", tw], ["7TV", th("7tv")], ["BTTV", th("bttv")], ["FFZ", th("ffz")]]; if (ch) ch.pk = out; return out;
  }
  function cell(e) {
    const b = el("button"); b.type = "button"; b.tabIndex = -1; b.ariaLabel = e.n; b._e = e; const im = el("img"); im.src = e.u; im.alt = ""; im.loading = "lazy"; im.decoding = "async"; im.draggable = false; b.appendChild(im); return b;
  }
  function build() {
    const q = search.value.trim().toLowerCase(), secs = sections(), lim = q ? 200 : 500; body.replaceChildren(); jump.replaceChildren(); let any = false;
    const rec = !q ? recent() : []; const all = rec.length ? [["Recent", rec]].concat(secs) : secs;
    for (const [name, arr0] of all) {
      let arr = q ? arr0.filter(e => e.n.toLowerCase().includes(q)) : arr0; if (!arr.length) continue; any = true;
      const sec = el("div", "lc-sec"); sec.dataset.s = name; const h = el("h4"); h.appendChild(el("span", "", name)); h.appendChild(el("span", "", String(arr.length))); sec.appendChild(h);
      const g = el("div", "lc-eg"); const frag = document.createDocumentFragment(); for (const e of arr.slice(0, lim)) frag.appendChild(cell(e)); g.appendChild(frag); sec.appendChild(g);
      if (arr.length > lim) sec.appendChild(el("div", "lc-more", (arr.length - lim) + " more, search to narrow down")); body.appendChild(sec);
      if (name !== "Recent") { const jb = el("button", "", name); jb.type = "button"; jb.onclick = () => sec.scrollIntoView({ block: "start" }); jump.appendChild(jb); }
    }
    if (!any) body.appendChild(el("div", "lc-none", q ? "No emote matches" : (chans.get(active) && chans.get(active).emReady ? "No emotes available" : "Loading emotes...")));
    foot.replaceChildren(el("span", "", "Shift+click keeps the picker open"));
  }
  function pick(e, keep) { insertAtCaret(e.n + " "); pushRecent(e); if (!keep) close(); else search.focus(); }
  function close(refocus = true) { if (!open) return; open = false; P.classList.remove("on"); X.emb.setAttribute("aria-expanded", "false"); document.removeEventListener("mousedown", outside, true); if (refocus) X.input.focus(); }
  function outside(e) { if (!P.contains(e.target) && !X.emb.contains(e.target)) close(false); }
  function openIt() { if (open || X.emb.disabled) return; open = true; search.value = ""; build(); P.classList.add("on"); X.emb.setAttribute("aria-expanded", "true"); document.addEventListener("mousedown", outside, true); search.focus(); }
  search.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(build, 80); });
  search.addEventListener("keydown", e => {
    if (e.key === "Enter") { e.preventDefault(); const b = body.querySelector(".lc-eg button"); if (b) pick(b._e, e.shiftKey); }
    else if (e.key === "ArrowDown") { e.preventDefault(); const b = body.querySelector(".lc-eg button"); if (b) b.focus(); }
  });
  P.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); return; }
    if (e.key === "Tab") { const f = [search, ...jump.querySelectorAll("button")]; const i = f.indexOf(document.activeElement);
      if (i < 0) { e.preventDefault(); search.focus(); } else { const n = (i + (e.shiftKey ? -1 : 1) + f.length) % f.length; e.preventDefault(); f[n].focus(); } return; }
    const b = e.target.closest && e.target.closest(".lc-eg button");
    if (b && /^Arrow/.test(e.key)) {
      e.preventDefault(); const btns = [...b.parentNode.parentNode.parentNode.querySelectorAll(".lc-eg button")], i = btns.indexOf(b); const cols = Math.max(1, Math.floor(b.parentNode.clientWidth / (b.offsetWidth + 3)));
      const d = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -cols, ArrowDown: cols }[e.key], t = btns[i + d]; if (t) t.focus(); else if (e.key === "ArrowUp") search.focus();
    }
  });
  body.addEventListener("click", e => { const b = e.target.closest(".lc-eg button"); if (b) pick(b._e, e.shiftKey); });
  const hover = e => { const b = e.target.closest && e.target.closest(".lc-eg button"); if (!b) return; const x = b._e; foot.replaceChildren(); foot.appendChild(el("b", "", x.n)); foot.appendChild(el("span", "", x.p + (x.zw ? ", overlay" : ""))); };
  body.addEventListener("mouseover", hover); body.addEventListener("focusin", hover);
  return { isOpen: () => open, open: openIt, close, build, toggle: () => (open ? close() : openIt()) };
}

/* ---------- DOM skeleton ---------- */
function buildUI(box) {
  const r = el("div", "lc"); r.hidden = true; r.dataset.density = prefs.density; r.classList.toggle("ts-on", !!prefs.timestamps); r.classList.toggle("nobadge", !prefs.badges);
  X.root = r; X.room = el("div", "lc-room"); X.banner = el("div", "lc-banner"); X.banner.setAttribute("role", "status");
  X.main = el("div", "lc-main");
  const sk = el("div", "lc-skel"); sk.appendChild(el("p", "", "Connecting to chat...")); [88, 62, 76, 48, 70].forEach(w => { const i = el("i"); i.style.width = w + "%"; sk.appendChild(i); }); X.main.appendChild(sk);
  X.pill = el("button", "lc-pill"); X.pill.type = "button"; X.pill.appendChild(svg(ICO.down)); X.pillT = el("span"); X.pill.appendChild(X.pillT);
  X.pill.onclick = () => { const ch = chans.get(active); if (ch) resume(ch); }; X.main.appendChild(X.pill);
  const comp = el("div", "lc-compose");
  X.err = el("div", "lc-err"); X.err.setAttribute("role", "alert"); X.reply = el("div", "lc-reply");
  const si = el("div", "lc-signin"), sd = el("div"); sd.appendChild(el("b", "", "Sign in to chat")); X.signinT = el("span", "", "Sending needs one more permission."); sd.appendChild(X.signinT); si.appendChild(sd);
  const sb = el("button", "", "Sign in again"); sb.type = "button"; sb.onclick = () => { try { O.onNeedSignIn && O.onNeedSignIn(); } catch (e) {} }; si.appendChild(sb);
  const iw = el("div", "lc-inp"); X.inpWrap = iw; const inp = el("input"); inp.type = "text"; inp.autocomplete = "off"; inp.spellcheck = true; inp.maxLength = 600; inp.placeholder = "Send a message";
  inp.setAttribute("role", "combobox"); inp.setAttribute("aria-autocomplete", "list"); inp.setAttribute("aria-controls", "lc-acbox"); inp.setAttribute("aria-expanded", "false"); X.input = inp;
  const emb = el("button"); emb.type = "button"; emb.title = emb.ariaLabel = "Emotes (Ctrl+E)"; emb.setAttribute("aria-haspopup", "dialog"); emb.setAttribute("aria-expanded", "false"); emb.appendChild(svg(ICO.smile)); X.emb = emb;
  const sd2 = el("button", "lc-send"); sd2.type = "button"; sd2.title = sd2.ariaLabel = "Send"; sd2.appendChild(svg(ICO.send)); sd2.disabled = true; X.send = sd2;
  X.counter = el("div", "lc-counter"); X.counter.setAttribute("aria-hidden", "true");
  iw.appendChild(inp); iw.appendChild(emb); iw.appendChild(sd2); iw.appendChild(X.counter);
  X.ac = el("div", "lc-ac"); X.ac.id = "lc-acbox"; X.ac.setAttribute("role", "listbox");
  const P = el("div", "lc-picker"); P.setAttribute("role", "dialog"); P.setAttribute("aria-label", "Emote picker"); X.picker = P;
  const ph = el("div", "lc-ph"), se = el("label", "lc-search"); se.appendChild(svg(ICO.search)); X.psearch = el("input"); X.psearch.type = "text"; X.psearch.placeholder = "Search emotes"; X.psearch.setAttribute("aria-label", "Search emotes"); se.appendChild(X.psearch);
  X.pjump = el("div", "lc-jump"); ph.appendChild(se); ph.appendChild(X.pjump); X.pbody = el("div", "lc-pbody"); X.pfoot = el("div", "lc-pf"); P.appendChild(ph); P.appendChild(X.pbody); P.appendChild(X.pfoot);
  comp.appendChild(X.err); comp.appendChild(X.reply); comp.appendChild(si); comp.appendChild(iw); comp.appendChild(X.ac); comp.appendChild(P);
  r.appendChild(X.room); r.appendChild(X.banner); r.appendChild(X.main); r.appendChild(comp); box.classList.add("lc-host"); box.appendChild(r);
  picker = makePicker();
  inp.addEventListener("input", onInput);
  inp.addEventListener("blur", () => setTimeout(acClose, 120));
  inp.addEventListener("click", acUpdate);
  inp.addEventListener("keydown", e => {
    if (e.isComposing) return; const k = e.key;
    if (ac.on) {
      if (k === "ArrowDown") { e.preventDefault(); return acMove(1); } if (k === "ArrowUp") { e.preventDefault(); return acMove(-1); }
      if (k === "Tab" || k === "Enter") { e.preventDefault(); return acAccept(); } if (k === "Escape") { e.preventDefault(); e.stopPropagation(); return acClose(); }
    }
    if (k === "Enter") { e.preventDefault(); if (!e.shiftKey) doSend(); return; }
    if (k === "Escape") { const ch = chans.get(active); if (picker.isOpen()) { picker.close(); return; } if (ch && ch.reply) { ch.reply = null; renderReply(ch); e.stopPropagation(); return; } inp.blur(); return; }
    if (k === "e" && e.ctrlKey) { e.preventDefault(); picker.toggle(); return; }
    if (k === "End" && !inp.value) { const ch = chans.get(active); if (ch) resume(ch); }
  });
  r.addEventListener("keydown", e => { if (e.key === "e" && e.ctrlKey && e.target !== inp) { e.preventDefault(); picker.toggle(); } });
  emb.onclick = () => picker.toggle(); sd2.onclick = doSend;
  document.addEventListener("visibilitychange", () => { if (!document.hidden) schedule(); });
}

/* ---------- public API ---------- */
function refreshStatus(force) {
  if (!force && (gs.scope === true || Date.now() - gs.at < 5000)) return Promise.resolve();
  gs.at = Date.now();
  return call("chat_status").then(s => { gs.scope = !!(s && s.has_scope); gs.me = s && s.me || null; if (active) updateCompose(); },
    () => { gs.scope = null; gs.me = null; if (active) updateCompose(); });
}
function init(o) {
  if (O) return; O = o; try { prefs = Object.assign(prefs, JSON.parse(lsGet("chat.prefs") || "{}")); } catch (e) {}
  buildUI(o.box);
  const lis = o.listen("chat-batch", ev => { const p = ev && ev.payload; if (!Array.isArray(p)) return; for (const it of p) onItem(it); schedule(); });
  refreshStatus(true);
  return lis;
}
function show(login) {
  if (!O || !login) return; login = String(login).toLowerCase(); clearTimeout(hideTimer);
  const prev = active && chans.get(active); if (prev) { prev.draft = X.input.value; prev.st = prev.list.scrollTop; prev.list.classList.add("off"); }
  if (picker && picker.isOpen()) picker.close(false); acClose(); showErr("");
  active = login; X.root.hidden = false;
  let ch = chans.get(login); if (!ch) ch = mkChan(login);
  warm = warm.filter(l => l !== login); warm.push(login);
  while (warm.length > WARM) leave(warm[0]);
  if (!ch.joined) join(ch);
  ch.list.classList.remove("off"); X.input.value = ch.draft || ""; onInput();
  if (ch.paused) { ch.list.scrollTop = ch.st; ch.last = -9; } else toBottom(ch);
  renderState(); refreshStatus(); schedule();
}
function hide() {
  if (!O) return; active = null; if (X.root) X.root.hidden = true; if (picker && picker.isOpen()) picker.close(false);
  clearTimeout(hideTimer); hideTimer = setTimeout(() => { for (const l of [...chans.keys()]) leave(l); }, 60000);
}
function refreshTheme() {
  if (!O) return; colCache.clear();
  for (const ch of chans.values()) for (const r of ch.list.children) if (r._c) { const u = r.querySelector(".u"); if (u) u.style.color = fixColor(r._c); if (r.classList.contains("action")) { const b = r.querySelector(".body"); if (b) b.style.color = fixColor(r._c); } }
}
function setPrefs(p) {
  Object.assign(prefs, p || {}); lsSet("chat.prefs", JSON.stringify(prefs)); if (!X.root) return;
  X.root.dataset.density = prefs.density; X.root.classList.toggle("ts-on", !!prefs.timestamps); X.root.classList.toggle("nobadge", !prefs.badges);
}
function focusInput() { if (X.input && !X.root.hidden) { X.input.focus(); return true; } return false; }
window.LurklyChat = { init, show, hide, refreshTheme, setPrefs, focusInput };
})();
