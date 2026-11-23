// Migrate Anything Studio — single-page app (no framework, no build step).
// Views: home → wizard (1 Connect, 2 Map) → plan (3 Plan, 4 Approve) → job (5 Run & proof).
import { CodeRain } from "/assets/rain.js";

// ============================================================ tiny DOM toolkit
const main = document.getElementById("main");
const SVG_NS = "http://www.w3.org/2000/svg";

function h(tag, attrs, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === false || v == null) continue;
    if (k === "class") el.className = v;
    else if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2), v);
    else if (k === "value") el.value = v;
    else if (k === "checked" || k === "disabled" || k === "selected" || k === "open") el[k] = !!v;
    else el.setAttribute(k, v === true ? "" : v);
  }
  for (const kid of kids.flat(Infinity)) {
    if (kid == null || kid === false) continue;
    el.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
  }
  return el;
}

/** Replace the children of `el`, skipping null/false (the DOM would print them as text). */
function put(el, ...kids) {
  el.replaceChildren(...kids.flat(Infinity).filter((k) => k != null && k !== false));
}
const mount = (...kids) => put(main, ...kids);

const ICONS = {
  check: "M20 6 9 17l-5-5",
  x: "M18 6 6 18M6 6l12 12",
  alert: "M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z",
  info: "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20zM12 16v-4M12 8h.01",
  folder: "M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.7-.9l-.8-1.2A2 2 0 0 0 7.9 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2z",
  database: "M12 8c4.97 0 9-1.34 9-3s-4.03-3-9-3-9 1.34-9 3 4.03 3 9 3zM3 5v14c0 1.66 4.03 3 9 3s9-1.34 9-3V5M3 12c0 1.66 4.03 3 9 3s9-1.34 9-3",
  globe: "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20zM2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z",
  play: "M6 3l14 9-14 9V3z",
  pause: "M6 4h4v16H6zM14 4h4v16h-4z",
  stop: "M5 5h14v14H5z",
  right: "M5 12h14M12 5l7 7-7 7",
  left: "M19 12H5M12 19l-7-7 7-7",
  refresh: "M3 12a9 9 0 0 1 15-6.7L21 8M21 3v5h-5M21 12a9 9 0 0 1-15 6.7L3 16M3 21v-5h5",
  shield: "M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z",
  terminal: "M4 17l6-6-6-6M12 19h8",
  key: "M15.5 7.5a4.5 4.5 0 1 1-4.2 6.1L3 22v-4h3v-3h3l2.3-2.3A4.5 4.5 0 0 1 15.5 7.5zM17 7h.01",
  plus: "M12 5v14M5 12h14",
  trash: "M3 6h18M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2",
  copy: "M9 9h11a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H9a2 2 0 0 1-2-2v-9a2 2 0 0 1 2-2zM5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1",
  bolt: "M13 2 3 14h9l-1 8 10-12h-9l1-8z",
  link: "M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.7 1.7M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7l1.7-1.7",
  shuffle: "M16 3h5v5M4 20 21 3M21 16v5h-5M15 15l6 6M4 4l5 5",
  list: "M9 3h6v4H9zM9 5H5v16h14V5h-4M8 12h8M8 16h8",
  clock: "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20zM12 6v6l4 2",
  file: "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6",
  up: "M18 15l-6-6-6 6",
};

function icon(name) {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "2");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  const p = document.createElementNS(SVG_NS, "path");
  p.setAttribute("d", ICONS[name] || ICONS.info);
  svg.append(p);
  return svg;
}

const fmt = (n) => (n == null ? "—" : Number(n).toLocaleString("en-US"));
const plural = (n, one, many) => `${fmt(n)} ${n === 1 ? one : many || one + "s"}`;
function duration(ms) {
  if (ms == null || ms < 0) return "—";
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}
function when(iso) {
  if (!iso) return "";
  const d = new Date(iso);
  return isNaN(d) ? "" : d.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

// ============================================================ API
class ApiError extends Error {
  constructor(message, cls, status) {
    super(message);
    this.cls = cls;
    this.status = status;
  }
}

async function api(method, path, body) {
  let resp;
  try {
    resp = await fetch(`/api${path}`, {
      method,
      credentials: "same-origin",
      headers: body === undefined ? {} : { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new ApiError("Migrate Anything is not answering. Is it still running in your terminal?", "Offline", 0);
  }
  let data = null;
  try {
    data = await resp.json();
  } catch {
    /* empty body */
  }
  if (!resp.ok) {
    const e = data && data.error;
    throw new ApiError((e && e.message) || `Request failed (${resp.status})`, (e && e.class) || "Error", resp.status);
  }
  return data;
}

// ============================================================ feedback: toasts, modal, busy buttons
const toasts = document.getElementById("toasts");
function toast(message, kind = "ok") {
  const t = h("div", { class: `toast ${kind}` }, icon(kind === "ok" ? "check" : "alert"), h("div", {}, message));
  toasts.append(t);
  setTimeout(() => t.remove(), kind === "ok" ? 4500 : 9000);
}

/** Run an async action from a button: disables it, shows a spinner, reports failures. */
async function busy(button, fn) {
  if (button.disabled) return;
  const label = [...button.childNodes];
  button.disabled = true;
  button.prepend(h("span", { class: "spinner", "aria-hidden": "true" }));
  try {
    return await fn();
  } catch (e) {
    toast(e.message || String(e), "bad");
  } finally {
    button.disabled = false;
    button.replaceChildren(...label);
  }
}

function modal(title, body, actions) {
  const previous = document.activeElement;
  const close = () => {
    backdrop.remove();
    document.removeEventListener("keydown", onKey);
    previous && previous.focus && previous.focus();
  };
  const onKey = (e) => e.key === "Escape" && close();
  const box = h("div", { class: "modal", role: "dialog", "aria-modal": "true", "aria-label": title }, h("h2", {}, title), body, h("div", { class: "actions" }, actions(close)));
  const backdrop = h("div", { class: "modal-backdrop", onclick: (e) => e.target === backdrop && close() }, box);
  document.body.append(backdrop);
  document.addEventListener("keydown", onKey);
  (box.querySelector("input, select, button") || box).focus();
  return close;
}

function confirmDialog(title, text, okLabel, danger) {
  return new Promise((resolve) => {
    modal(title, h("p", { class: "dim" }, text), (close) => [
      h("button", { class: "btn", type: "button", onclick: () => (close(), resolve(false)) }, "Keep going"),
      h("button", { class: `btn ${danger ? "danger" : "primary"}`, type: "button", onclick: () => (close(), resolve(true)) }, okLabel),
    ]);
  });
}

function notice(kind, ...content) {
  const ic = { ok: "check", warn: "alert", bad: "alert", info: "info" }[kind] || "info";
  return h("div", { class: `notice ${kind}`, role: kind === "bad" ? "alert" : null }, icon(ic), h("div", { class: "grow" }, content));
}

function badge(kind, text, ic) {
  return h("span", { class: `badge ${kind}` }, ic ? icon(ic) : null, text);
}

function field(label, control, help, id) {
  const fid = id || `f${Math.random().toString(36).slice(2, 9)}`;
  control.id = control.id || fid;
  return h("div", { class: "field" }, h("label", { for: control.id }, label), control, help ? h("div", { class: "help" }, help) : null);
}

// ============================================================ state
const S = { info: null, wiz: freshWizard(), bgRain: null, motion: true };
function freshWizard() {
  return { step: 1, source: null, destination: null, disc: {}, discErr: {}, map: null, mapKey: null, pendingMapping: null, demo: null, adding: {}, choosing: {}, built: {} };
}

async function loadInfo() {
  S.info = await api("GET", "/state");
  const pill = document.getElementById("approval-pill");
  pill.replaceChildren(
    S.info.approvals.enabled
      ? h("span", { class: "badge ok", title: S.info.approvals.how }, icon("shield"), "Approvals: code in terminal")
      : h("span", { class: "badge warn", title: S.info.approvals.how }, icon("terminal"), "Approvals: command line"),
  );
  return S.info;
}
const connection = (name) => (S.info.connections || []).find((c) => c.name === name);
const kindIcon = (kind) => ({ files: "folder", postgres: "database", rest: "globe" })[kind] || "link";
const kindLabel = (kind) => ({ files: "Files", postgres: "PostgreSQL", rest: "Web API" })[kind] || kind;
function connDetail(c) {
  if (c.kind === "files") return `${c.settings.path} · ${String(c.settings.format || "").toUpperCase()}`;
  if (c.kind === "postgres") return c.settings.url;
  if (c.kind === "rest") return c.settings.base_url;
  return "";
}

// ============================================================ router
let routeToken = 0;
let cleanup = null;
const routes = [
  [/^#\/?$/, viewHome, "home"],
  [/^#\/new$/, viewWizard, "new"],
  [/^#\/demo$/, viewDemo, "new"],
  [/^#\/plan\/([\w-]+)(\/approve)?$/, viewPlan, "new"],
  [/^#\/job\/([\w-]+)(\/replay)?$/, viewJob, "new"],
];
async function route() {
  if (cleanup) {
    try {
      cleanup();
    } catch {
      /* ignore */
    }
    cleanup = null;
  }
  const token = ++routeToken;
  const hash = location.hash || "#/";
  const hit = routes.map(([re, fn, nav]) => [hash.match(re), fn, nav]).find(([m]) => m);
  if (!hit) {
    location.hash = "#/";
    return;
  }
  document.querySelectorAll(".navlink").forEach((a) => (a.dataset.nav === hit[2] ? a.setAttribute("aria-current", "page") : a.removeAttribute("aria-current")));
  S.bgRain && S.bgRain.setIntensity(0.34);
  try {
    const alive = () => token === routeToken;
    cleanup = (await hit[1](alive, ...hit[0].slice(1))) || null;
  } catch (e) {
    mount(notice("bad", h("strong", {}, "Something went wrong. "), e.message || String(e)));
  }
  window.scrollTo(0, 0);
}
const go = (hash) => (location.hash === hash ? route() : (location.hash = hash));

function stepper(current) {
  const steps = ["Connect", "Map", "Plan", "Approve", "Run & proof"];
  return [
    h("p", { class: "step-count" }, `Step ${current} of ${steps.length}`),
    h(
      "ol",
      { class: "stepper", "aria-label": "Migration steps" },
      steps.map((label, i) => {
        const n = i + 1;
        const cls = n < current ? "done" : n === current ? "current" : "";
        return h("li", { class: cls, "aria-current": n === current ? "step" : null }, h("span", { class: "num" }, n < current ? icon("check") : n), h("span", {}, label));
      }),
    ),
  ];
}

// ============================================================ home
async function viewHome(alive) {
  await loadInfo();
  if (!alive()) return;
  const jobs = S.info.jobs || [];
  const conns = S.info.connections || [];
  const statusBadge = (s) => badge({ completed: "ok", failed: "bad", cancelled: "warn", paused: "warn" }[s] || "info", s, { completed: "check", failed: "alert", running: "play", paused: "pause" }[s]);

  const demoBtn = h("button", { class: "btn big", type: "button", onclick: () => busy(demoBtn, startDemo) }, icon("bolt"), "Try the demo");
  mount(
    h(
      "section",
      { class: "hero" },
      h("h1", {}, "Move your data from anything to anything.", h("span", { class: "glow" }, "Then watch it arrive.")),
      h("p", {}, "Connect two systems, check how the fields line up, approve, and follow every record as it lands. When it is done you get proof that it all arrived."),
      h("div", { class: "row" }, h("a", { class: "btn primary big", href: "#/new", onclick: () => (S.wiz = freshWizard()) }, icon("right"), "Start a migration"), demoBtn),
      h("p", { class: "faint", style: "margin-top:12px" }, "The demo uses made-up data on this computer. Nothing leaves your machine."),
    ),
    h(
      "div",
      { class: "stack-lg" },
      h(
        "section",
        { class: "grid-3 howto", "aria-label": "How it works" },
        [
          ["link", "1 · Connect", "Point at where the data is and where it should go: folders of files, a database, or a web API."],
          ["shuffle", "2 · Map", "See which field goes where. Good matches are made for you; anything uncertain is flagged."],
          ["list", "3 · Plan", "Read exactly what will happen, with a test run that writes nothing."],
          ["shield", "4 · Approve", "Only you can approve, with a one-time code from your terminal."],
          ["play", "5 · Run & proof", "Watch it move, pause any time, then get a verified report."],
        ].map(([ic, title, text]) => h("div", { class: "panel tight" }, h("h3", {}, icon(ic), title), h("p", { class: "dim" }, text))),
      ),
      h(
        "section",
        { class: "grid-2" },
        h(
          "div",
          { class: "panel" },
          h("div", { class: "panel-title" }, h("h2", {}, icon("clock"), "Recent migrations")),
          jobs.length
            ? h(
                "ul",
                { class: "list" },
                jobs.map((j) => h("li", {}, statusBadge(j.status), h("span", { class: "grow" }, h("a", { href: `#/job/${j.job_id}` }, j.job_id), h("div", { class: "faint" }, when(j.created_at))))),
              )
            : h("div", { class: "empty" }, icon("clock"), "No migrations yet.", h("div", { class: "faint" }, "Your runs will be listed here so you can come back to their reports.")),
        ),
        h(
          "div",
          { class: "panel" },
          h("div", { class: "panel-title" }, h("h2", {}, icon("link"), "Connections")),
          conns.length
            ? h(
                "ul",
                { class: "list" },
                conns.map((c) => h("li", {}, badge("info", kindLabel(c.kind), kindIcon(c.kind)), h("span", { class: "grow" }, h("strong", {}, c.name), h("div", { class: "faint" }, connDetail(c))), badge(c.environment === "production" ? "warn" : "ok", c.environment))),
              )
            : h("div", { class: "empty" }, icon("link"), "No connections yet.", h("div", { class: "faint" }, "You add them in step 1 of a migration.")),
        ),
      ),
    ),
  );
}

/** `migrate-anything demo` opens this route: set the demo up and go straight to the mapping. */
async function viewDemo() {
  mount(h("div", { class: "row dim" }, h("span", { class: "spinner" }), "Preparing the demo data…"));
  await startDemo();
}

async function startDemo() {
  const d = await api("POST", "/demo");
  await loadInfo();
  S.wiz = freshWizard();
  S.wiz.source = d.source;
  S.wiz.destination = d.destination;
  S.wiz.demo = d;
  S.wiz.step = 2;
  toast(`Demo ready: ${fmt(d.records)} made-up CRM records.`);
  go("#/new");
}

// ============================================================ wizard: step 1 (connect) & 2 (map)
async function viewWizard(alive) {
  await loadInfo();
  if (!alive()) return;
  return S.wiz.step === 2 && S.wiz.source && S.wiz.destination ? renderMap(alive) : renderConnect(alive);
}

async function discover(name, force) {
  if (!force && S.wiz.disc[name]) return S.wiz.disc[name];
  delete S.wiz.discErr[name];
  try {
    S.wiz.disc[name] = await api("POST", `/connections/${encodeURIComponent(name)}/discover`);
  } catch (e) {
    delete S.wiz.disc[name];
    S.wiz.discErr[name] = e;
    throw e;
  }
  return S.wiz.disc[name];
}

function renderConnect(alive) {
  const rerender = () => alive() && renderConnect(alive);
  const w = S.wiz;
  const ready = w.source && w.destination && w.source !== w.destination && w.disc[w.source] && w.disc[w.destination];
  const next = h("button", { class: "btn primary big", type: "button", disabled: !ready, onclick: () => ((w.step = 2), route()) }, "Continue to mapping", icon("right"));
  mount(
    ...stepper(1),
    h("h1", {}, "Where is the data, and where should it go?"),
    h("p", { class: "dim", style: "margin:8px 0 24px" }, "Pick a connection on each side, or add a new one. Connecting only reads; nothing is written until you approve in step 4."),
    h("div", { class: "grid-2" }, side("source", rerender), side("destination", rerender)),
    w.source && w.source === w.destination ? h("div", { style: "margin-top:16px" }, notice("warn", "Source and destination are the same connection. Pick two different ones.")) : null,
    h("div", { class: "summary-bar" }, h("span", { class: "dim grow" }, ready ? "Both sides are connected." : "Connect both sides to continue."), h("a", { class: "btn ghost", href: "#/" }, "Cancel"), next),
  );
}

function side(role, rerender) {
  const w = S.wiz;
  const selected = w[role];
  const conns = S.info.connections || [];
  const other = w[role === "source" ? "destination" : "source"];
  const select = async (name) => {
    w[role] = name;
    w.map = null;
    delete w.choosing[role];
    rerender();
    try {
      await discover(name);
    } catch {
      /* shown inline */
    }
    rerender();
  };
  const body = h("div", { class: "stack" });
  body.append(
    h("div", { class: "side-title" }, h("span", { class: "tag" }, role === "source" ? "FROM" : "TO"), h("h2", {}, role === "source" ? "Where the data is now" : "Where it should go")),
  );
  const cardBody = (c) => [
    icon(kindIcon(c.kind)),
    h("span", { class: "grow" }, h("strong", {}, c.name), h("span", { class: "meta" }, `${kindLabel(c.kind)} · ${connDetail(c)}`)),
    badge(c.environment === "production" ? "warn" : "ok", c.environment),
  ];
  const chosen = selected && connection(selected);
  if (chosen && !w.choosing[role] && !w.adding[role]) {
    // one connection is chosen: show just that one, with what was found right under it
    body.append(
      h("div", { class: "conn-card chosen" }, cardBody(chosen), h("button", { class: "btn small ghost", type: "button", "aria-label": `Change the ${role} connection`, onclick: () => ((w.choosing[role] = true), rerender()) }, "Change")),
      discoveryPanel(role, selected, rerender),
    );
    return h("section", { class: "panel", "aria-label": role }, body);
  }
  if (conns.length) {
    body.append(
      h(
        "div",
        { class: "stack", role: "group", "aria-label": `${role} connection` },
        conns.map((c) => {
          const taken = c.name === other;
          return h(
            "button",
            { class: "conn-card", type: "button", disabled: taken, "aria-pressed": String(selected === c.name), "aria-label": `${c.name}, ${kindLabel(c.kind)}, ${c.environment}${taken ? ", already used on the other side" : ""}`, title: taken ? "Already used on the other side" : "", onclick: () => select(c.name) },
            cardBody(c),
          );
        }),
      ),
    );
  } else {
    body.append(h("p", { class: "dim" }, "No connections yet. Add your first one below."));
  }
  if (w.adding[role]) {
    body.append(connectionForm(role, rerender, select));
  } else {
    body.append(h("button", { class: "btn", type: "button", onclick: () => ((w.adding[role] = { kind: "files", fresh: true }), rerender()) }, icon("plus"), "Add a new connection"));
  }
  return h("section", { class: "panel", "aria-label": role }, body);
}

function discoveryPanel(role, name, rerender) {
  const w = S.wiz;
  const d = w.disc[name];
  const err = w.discErr[name];
  const c = connection(name);
  if (err) {
    const needsConnector = c && c.kind === "rest" && /no connector yet/.test(err.message);
    return h(
      "div",
      { class: "stack" },
      needsConnector ? notice("info", h("strong", {}, "This API needs a connector. "), "Give Migrate Anything the API's description and it builds one.") : notice("bad", h("strong", {}, "Could not connect. "), err.message),
      needsConnector ? connectorForm(name, rerender) : h("button", { class: "btn small", type: "button", onclick: async (e) => busy(e.currentTarget, async () => (await discover(name, true).catch(() => {}), rerender())) }, icon("refresh"), "Try again"),
    );
  }
  if (!d) return h("div", { class: "row dim" }, h("span", { class: "spinner" }), "Connecting…");
  const objs = d.schema.objects || [];
  const out = h(
    "div",
    { class: "stack" },
    notice("ok", h("strong", {}, "Connected. "), `${d.identity.name} — ${plural(objs.length, "data set")} found.`),
    objs.length
      ? h("div", { class: "objects" }, objs.map((o) => h("span", { class: "badge" }, `${o.name} · ${plural(o.fields.length, "field")}${o.estimated_count != null ? ` · ${fmt(o.estimated_count)} records` : ""}`)))
      : role === "destination"
        ? h("p", { class: "dim" }, "Nothing here yet — the migration will create it.")
        : notice("warn", "No data was found here. Check the folder and the file type."),
  );
  if (w.built[name]) out.append(...builtSummary(w.built[name]));
  // file sources: the ID column is guessed per data set; one click confirms all of them
  if (c && c.kind === "files" && role === "source") {
    const missing = objs.filter((o) => !o.primary_key || !o.primary_key.length);
    if (missing.length) {
      const picks = missing.map((o) => {
        const guess = guessIdColumn(o);
        const sel = h("select", {}, h("option", { value: "" }, "No ID column"), o.fields.map((f) => h("option", { value: f.name, selected: f.name === guess }, f.name)));
        return { object: o.name, sel, guess };
      });
      const guessed = picks.filter((x) => x.guess).length;
      const save = h("button", { class: "btn primary", type: "button", onclick: (e) => busy(e.currentTarget, () => saveIdFields(c, Object.fromEntries(picks.filter((x) => x.sel.value).map((x) => [x.object, x.sel.value])), rerender)) }, icon("check"), "Use these ID columns");
      out.append(
        notice(
          "warn",
          h("strong", {}, "Which column identifies each record? "),
          guessed === picks.length ? "These look right. Check them and confirm." : "An ID column lets Migrate Anything match records, keep relationships and verify the result.",
          h("div", { class: "form-grid", style: "margin-top:12px" }, picks.map((x) => field(x.object, x.sel))),
          h("div", { style: "margin-top:12px" }, save),
        ),
      );
    }
  }
  const reg = c && c.connector_ref ? (S.info.connectors || []).find((r) => r.reference === c.connector_ref) : null;
  if (reg && role === "destination" && reg.status !== "active") out.append(activationPanel(reg.reference, rerender));
  return out;
}

/** The column most likely to be the record ID: "id", "<object>_id", or an "_id" column whose
 *  prefix abbreviates the data set name (org_id for organizations). Null when nothing fits. */
function guessIdColumn(o) {
  const names = o.fields.map((f) => f.name);
  const lower = (x) => x.toLowerCase();
  const obj = lower(o.name).replace(/[^a-z0-9]/g, "");
  const singular = obj.replace(/ies$/, "y").replace(/s$/, "");
  const exact = names.find((n) => ["id", "uuid", "key", `${obj}id`, `${singular}id`].includes(lower(n).replace(/[^a-z0-9]/g, "")));
  if (exact) return exact;
  const abbreviated = names.filter((n) => {
    const m = /^(.+?)[_-]?id$/i.exec(n);
    return m && m[1].length >= 2 && obj.startsWith(lower(m[1]).replace(/[^a-z0-9]/g, ""));
  });
  return abbreviated.length === 1 ? abbreviated[0] : null;
}

async function saveIdFields(c, chosen, rerender) {
  if (!Object.keys(chosen).length) throw new Error("Choose the ID column for at least one data set.");
  const settings = { ...c.settings, id_fields: { ...(c.settings.id_fields || {}), ...chosen } };
  await api("POST", "/connections", { name: c.name, kind: c.kind, environment: c.environment, settings });
  await loadInfo();
  await discover(c.name, true);
  toast("ID columns saved.");
  rerender();
}

/** What building a connector from an API description produced: data sets, warnings, read tests. */
function builtSummary(r) {
  const g = r.generated;
  const odd = (g.suspicious_text || []).length;
  return [
    notice("ok", h("strong", {}, `Connector ${g.connector_ref} built. `), `${plural(g.spec.resources.length, "data set")}: ${g.spec.resources.map((x) => x.name).join(", ")}.`),
    odd ? notice("warn", h("strong", {}, "Heads-up: "), `the API's documentation contains ${plural(odd, "passage")} written as instructions to AI tools. ${odd === 1 ? "It was" : "They were"} ignored and left out.`) : null,
    r.test
      ? h("details", { open: !r.test.passed }, h("summary", {}, badge(r.test.passed ? "ok" : "bad", r.test.passed ? `${r.test.results.length} read tests passed` : "some read tests failed", r.test.passed ? "check" : "alert")), h("div", { class: "body" }, h("ul", {}, r.test.results.map((t) => h("li", {}, `${t.passed ? "pass" : "FAIL"} — ${t.name}: ${t.detail}`)))))
      : notice("bad", h("strong", {}, "The connector could not be tested. "), r.test_error || ""),
  ].filter(Boolean);
}

function connectorForm(name, rerender) {
  // most APIs publish their description at /openapi.json: offer that as the starting point
  const c = connection(name);
  const likely = c && c.settings.base_url ? `${new URL(c.settings.base_url).origin}/openapi.json` : "";
  const url = h("input", { type: "text", placeholder: "https://api.example.com/openapi.json", value: likely, autocomplete: "off" });
  const out = h("div", { class: "stack" });
  const run = async (btn) =>
    busy(btn, async () => {
      const v = url.value.trim();
      if (!v) throw new Error("Enter the address (or file path) of the API description.");
      const body = /^https?:\/\//i.test(v) ? { openapi_url: v } : { openapi_file: v };
      const r = await api("POST", `/connections/${encodeURIComponent(name)}/connector`, body);
      await loadInfo();
      S.wiz.built[name] = r; // kept, so the result stays visible once the connection is live
      out.replaceChildren(...builtSummary(r));
      await discover(name, true).catch(() => {});
      rerender();
    });
  return h(
    "div",
    { class: "stack" },
    field("API description (OpenAPI) — web address or file path", url, likely ? "This is the usual place. If the build fails, look in the API's documentation for its OpenAPI or Swagger link." : "Most APIs publish one, often at /openapi.json or /swagger.json."),
    h("button", { class: "btn primary", type: "button", onclick: (e) => run(e.currentTarget) }, icon("bolt"), "Build the connector"),
    out,
  );
}

function activationPanel(reference, rerender) {
  if (!S.info.approvals.enabled) {
    return notice("warn", h("strong", {}, "This connector must be activated before anything can be written through it. "), "Run this in a terminal:", h("div", { class: "cmd", style: "margin-top:8px" }, h("code", {}, `migrate-anything connectors activate ${reference}`)));
  }
  return notice(
    "warn",
    h("strong", {}, "Activate this connector to allow writing through it. "),
    "It was generated automatically, so you confirm it once.",
    codeFlow({ kind: "connector", subject: reference, label: "Activate connector", submit: (challenge_id, code) => api("POST", "/connectors/activate", { reference, challenge_id, code }), done: async () => (toast("Connector activated."), await loadInfo(), rerender()) }),
  );
}

/** Shared "code from the terminal" flow used for plan approval and connector activation. */
function codeFlow({ kind, subject, label, submit, done, extra }) {
  const box = h("div", { class: "stack", style: "margin-top:12px" });
  const start = h("button", { class: "btn primary", type: "button" }, icon("terminal"), "Show a code in my terminal");
  const ask = () =>
    busy(start, async () => {
      const ch = await api("POST", "/challenges", { kind, subject });
      const input = h("input", { class: "code-input", type: "text", inputmode: "text", autocomplete: "one-time-code", maxlength: "12", "aria-label": "Approval code from the terminal", placeholder: "······" });
      const errBox = h("div", { class: "field" });
      const okBtn = h("button", { class: "btn primary big", type: "button" }, icon("shield"), label);
      const send = () =>
        busy(okBtn, async () => {
          errBox.replaceChildren();
          if (input.value.trim().length < 6) {
            errBox.replaceChildren(h("div", { class: "error", role: "alert" }, icon("alert"), "Type the 6-character code shown in the terminal."));
            input.setAttribute("aria-invalid", "true");
            input.focus();
            return;
          }
          try {
            await submit(ch.challenge_id, input.value.trim());
          } catch (e) {
            errBox.replaceChildren(h("div", { class: "error", role: "alert" }, icon("alert"), e.message));
            input.setAttribute("aria-invalid", "true");
            input.select();
            return;
          }
          await done();
        });
      okBtn.addEventListener("click", send);
      input.addEventListener("keydown", (e) => e.key === "Enter" && send());
      box.replaceChildren(
        h("div", { class: "terminal-hint" }, "Look at the terminal window where Migrate Anything is running.\nIt now shows a 6-character code. Type it here."),
        field("Code from the terminal", input, "The code works once and expires after 5 minutes."),
        errBox,
        ...(extra ? [extra] : []),
        h("div", { class: "row" }, okBtn, h("button", { class: "btn ghost", type: "button", onclick: ask }, icon("refresh"), "Send a new code")),
      );
      input.focus();
    });
  start.addEventListener("click", ask);
  box.append(start);
  return box;
}

function connectionForm(role, rerender, select) {
  const draft = S.wiz.adding[role];
  const kinds = [
    ["files", "Files", "folder"],
    ["postgres", "PostgreSQL", "database"],
    ["rest", "Web API", "globe"],
  ];
  const seg = h(
    "div",
    { class: "segmented", role: "group", "aria-label": "Kind of system" },
    kinds.map(([k, label, ic]) => h("button", { type: "button", "aria-pressed": String(draft.kind === k), onclick: () => ((draft.kind = k), rerender()) }, icon(ic), label)),
  );
  const name = h("input", { type: "text", placeholder: role === "source" ? "old-crm" : "new-system", value: draft.name || "", autocomplete: "off", oninput: (e) => (draft.name = e.target.value) });
  const env = h(
    "select",
    { onchange: (e) => (draft.environment = e.target.value) },
    [
      ["production", "Production (real data)"],
      ["staging", "Staging"],
      ["test", "Test (safe to experiment)"],
    ].map(([v, l]) => h("option", { value: v, selected: (draft.environment || "production") === v }, l)),
  );
  const errors = h("div", { class: "stack" });
  const specific = h("div", { class: "form-grid" });
  const get = {};

  if (draft.kind === "files") {
    const path = h("input", { type: "text", placeholder: "C:\\data\\export", value: draft.path || "", autocomplete: "off", oninput: (e) => (draft.path = e.target.value) });
    const format = h(
      "select",
      { onchange: (e) => (draft.format = e.target.value) },
      [
        ["csv", "CSV (.csv)"],
        ["jsonl", "JSON Lines (.jsonl)"],
        ["json", "JSON array (.json)"],
      ].map(([v, l]) => h("option", { value: v, selected: (draft.format || (role === "destination" ? "jsonl" : "csv")) === v }, l)),
    );
    const browse = h("button", { class: "btn", type: "button", onclick: () => folderPicker(path.value, (p, counts) => ((path.value = p), (draft.path = p), autoFormat(counts))) }, icon("folder"), "Browse…");
    const autoFormat = (counts) => {
      const best = Object.entries(counts || {}).sort((a, b) => b[1] - a[1])[0];
      if (best) {
        format.value = best[0];
        draft.format = best[0];
      }
    };
    specific.append(
      h("div", { class: "field", style: "grid-column:1/-1" }, h("label", { for: "f-path-" + role }, "Folder"), h("div", { class: "row" }, Object.assign(path, { id: "f-path-" + role }), browse), h("div", { class: "help" }, role === "source" ? "A folder with one file per data set, e.g. customers.csv, orders.csv." : "A folder for the new files. It is created if it does not exist.")),
      field("File type", format),
    );
    get.settings = () => {
      if (!path.value.trim()) throw new Error("Choose the folder.");
      return { path: path.value.trim(), format: format.value };
    };
  } else if (draft.kind === "postgres") {
    const mk = (key, ph, type = "text") => h("input", { type, placeholder: ph, value: draft[key] || "", autocomplete: "off", oninput: (e) => (draft[key] = e.target.value) });
    const host = mk("host", "localhost");
    const port = mk("port", "5432");
    const db = mk("db", "mydatabase");
    const user = mk("user", "postgres");
    const pass = mk("secret", "", "password");
    const schema = mk("schema", "public");
    specific.append(field("Server", host), field("Port", port), field("Database", db), field("User", user), field("Password", pass, "Stored encrypted on this computer. Never shown again."), field("Schema", schema, "Usually public."));
    get.settings = () => {
      if (!db.value.trim() || !user.value.trim()) throw new Error("Enter at least the database name and the user.");
      const url = `postgres://${encodeURIComponent(user.value.trim())}@${host.value.trim() || "localhost"}:${port.value.trim() || "5432"}/${encodeURIComponent(db.value.trim())}`;
      return { url, schema: schema.value.trim() || "public" };
    };
    get.secret = () => pass.value;
  } else {
    const base = h("input", { type: "text", placeholder: "https://api.example.com", value: draft.base || "", autocomplete: "off", oninput: (e) => (draft.base = e.target.value) });
    const key = h("input", { type: "password", value: draft.secret || "", autocomplete: "off", oninput: (e) => (draft.secret = e.target.value) });
    const local = h("input", { type: "checkbox", checked: !!draft.local, onchange: (e) => (draft.local = e.target.checked) });
    specific.append(
      h("div", { style: "grid-column:1/-1" }, field("API address", base, "Only this server will ever be contacted for this connection.")),
      field("API key or token", key, "Stored encrypted on this computer. Leave empty if the API needs none."),
      h("label", { class: "check", style: "grid-column:1/-1" }, local, h("span", {}, "This API runs on my own computer or private network", h("div", { class: "help" }, "Leave off for normal internet APIs. Private addresses are blocked unless you tick this."))),
    );
    get.settings = () => {
      if (!/^https?:\/\//i.test(base.value.trim())) throw new Error("The API address must start with http:// or https://");
      return { base_url: base.value.trim().replace(/\/+$/, ""), allow_private_network: local.checked };
    };
    get.secret = () => key.value;
  }

  const save = h("button", { class: "btn primary", type: "submit" }, icon("link"), "Save and connect");
  const form = h(
    "form",
    {
      class: "stack panel tight",
      novalidate: true,
      onsubmit: (e) => {
        e.preventDefault();
        busy(save, async () => {
          errors.replaceChildren();
          const n = name.value.trim();
          try {
            if (!/^[A-Za-z0-9_-]{1,64}$/.test(n)) throw new Error("Give the connection a short name using letters, digits, - or _ (for example old-crm).");
            const settings = get.settings();
            await api("POST", "/connections", { name: n, kind: draft.kind, environment: env.value, settings, secret: get.secret ? get.secret() : undefined });
          } catch (err) {
            errors.replaceChildren(notice("bad", h("strong", { tabindex: "-1" }, "Please fix this: "), err.message));
            errors.querySelector("strong").focus();
            return;
          }
          delete S.wiz.adding[role];
          await loadInfo();
          toast(`Connection ${n} saved.`);
          await select(n);
        });
      },
    },
    h("h3", {}, "New connection"),
    seg,
    errors,
    h("div", { class: "form-grid" }, field("Name", name, "Your own label for this system."), field("What kind of environment is it?", env)),
    specific,
    h("div", { class: "row" }, save, h("button", { class: "btn ghost", type: "button", onclick: () => (delete S.wiz.adding[role], rerender()) }, "Cancel")),
  );
  if (draft.fresh) {
    delete draft.fresh;
    requestAnimationFrame(() => {
      form.scrollIntoView({ block: "center", behavior: S.motion ? "smooth" : "auto" });
      name.focus({ preventScroll: true });
    });
  }
  return form;
}

function folderPicker(startPath, onPick) {
  const list = h("div", { class: "dir-list" });
  const here = h("p", { class: "mono", style: "overflow-wrap:anywhere" });
  const info = h("p", { class: "dim" });
  let current = null;
  const load = async (path) => {
    list.replaceChildren(h("div", { class: "row dim", style: "padding:12px" }, h("span", { class: "spinner" }), "Reading…"));
    try {
      current = await api("GET", `/fs?path=${encodeURIComponent(path || "")}`);
    } catch (e) {
      list.replaceChildren(h("div", { style: "padding:12px" }, notice("bad", e.message)));
      return;
    }
    here.textContent = current.path;
    const counts = Object.entries(current.data_files || {});
    info.textContent = counts.length ? `Data files here: ${counts.map(([ext, n]) => `${n} ${ext.toUpperCase()}`).join(", ")}` : "No CSV or JSON files directly in this folder.";
    list.replaceChildren(
      ...(current.parent ? [h("button", { type: "button", onclick: () => load(current.parent) }, icon("up"), "Up one level")] : []),
      ...(current.roots || []).filter((r) => !current.path.startsWith(r) || current.parent == null).map((r) => h("button", { type: "button", onclick: () => load(r) }, icon("database"), r)),
      ...current.dirs.map((d) => h("button", { type: "button", onclick: () => load(d.path) }, icon("folder"), d.name)),
    );
  };
  modal("Choose a folder", h("div", { class: "stack" }, here, info, list), (close) => [
    h("button", { class: "btn", type: "button", onclick: close }, "Cancel"),
    h("button", { class: "btn primary", type: "button", onclick: () => current && (onPick(current.path, current.data_files), close()) }, icon("check"), "Use this folder"),
  ]);
  load(startPath);
}

// ------------------------------------------------------------ step 2: mapping
const TRANSFORMS = [
  ["copy", "Copy as it is"],
  ["cast", "Convert type…"],
  ["upper", "UPPER CASE"],
  ["lower", "lower case"],
  ["trim", "Trim spaces"],
  ["ref", "Link to another data set…"],
  ["scale", "Shift decimal point…"],
  ["parse_date", "Read as date…"],
  ["enum", "Translate values…"],
  ["const", "Always the same value…"],
  ["advanced", "Advanced (JSON)"],
];
const TYPES = [
  ["string", "text"],
  ["int", "whole number"],
  ["decimal", "decimal number"],
  ["float", "floating point"],
  ["bool", "yes / no"],
  ["date", "date"],
  ["date_time", "date and time"],
];
const valText = (v) => (!v || v.type === "null" ? "" : String(v.value ?? ""));
const fieldPath = (e) => (e && e.op === "field" ? e.path : null);
function firstPath(e) {
  if (!e || typeof e !== "object") return null;
  if (e.op === "field") return e.path;
  for (const v of Object.values(e)) {
    const p = Array.isArray(v) ? v.map(firstPath).find(Boolean) : firstPath(v);
    if (p) return p;
  }
  return null;
}
function exprToRow(e) {
  const src = fieldPath(e.input);
  switch (e.op) {
    case "field":
      return { kind: "copy", source: e.path, params: {} };
    case "cast":
      if (src) return { kind: "cast", source: src, params: { to: e.to } };
      break;
    case "upper":
    case "lower":
    case "trim":
      if (src) return { kind: e.op, source: src, params: {} };
      break;
    case "ref":
      if (src) return { kind: "ref", source: src, params: { object: e.object } };
      break;
    case "scale_decimal":
      if (src) return { kind: "scale", source: src, params: { pow: e.pow } };
      break;
    case "parse_date":
      if (src) return { kind: "parse_date", source: src, params: { format: e.format, datetime: !!e.datetime } };
      break;
    case "enum_map":
      if (src) return { kind: "enum", source: src, params: { pairs: Object.entries(e.map || {}).map(([k, v]) => `${k}=${valText(v)}`).join("\n"), on_unmapped: e.on_unmapped || "fail" } };
      break;
    case "const":
      return { kind: "const", source: null, params: { value: valText(e.value) } };
  }
  return { kind: "advanced", source: firstPath(e), params: { json: JSON.stringify(e, null, 1) } };
}
function rowToExpr(row) {
  const f = () => {
    if (!row.source) throw new Error(`"${row.target}": choose which source field it comes from.`);
    return { op: "field", path: row.source };
  };
  const p = row.params || {};
  switch (row.kind) {
    case "copy":
      return f();
    case "cast":
      return { op: "cast", input: f(), to: p.to || "string" };
    case "upper":
    case "lower":
    case "trim":
      return { op: row.kind, input: f() };
    case "ref":
      if (!p.object) throw new Error(`"${row.target}": choose which data set it links to.`);
      return { op: "ref", input: f(), object: p.object };
    case "scale": {
      const pow = parseInt(p.pow, 10);
      if (Number.isNaN(pow)) throw new Error(`"${row.target}": enter how many places to shift (for example 2).`);
      return { op: "scale_decimal", input: f(), pow };
    }
    case "parse_date":
      if (!p.format) throw new Error(`"${row.target}": enter the date pattern (for example %d.%m.%Y).`);
      return { op: "parse_date", input: f(), format: p.format, datetime: !!p.datetime };
    case "enum": {
      const map = {};
      for (const line of String(p.pairs || "").split("\n")) {
        const i = line.indexOf("=");
        if (i > 0) map[line.slice(0, i).trim()] = { type: "string", value: line.slice(i + 1).trim() };
      }
      if (!Object.keys(map).length) throw new Error(`"${row.target}": add at least one translation, one per line, like A=active.`);
      return { op: "enum_map", input: f(), map, on_unmapped: p.on_unmapped || "fail" };
    }
    case "const":
      return { op: "const", value: { type: "string", value: String(p.value ?? "") } };
    case "advanced":
      try {
        return JSON.parse(p.json);
      } catch {
        throw new Error(`"${row.target}": the advanced rule is not valid JSON.`);
      }
  }
  throw new Error(`"${row.target}": unknown rule.`);
}

function buildMapModel(resp, pending) {
  const src = resp.source.schema;
  const dst = resp.destination.schema;
  const creates = resp.destination_creates_objects;
  const spec = pending || resp.suggestion.mapping;
  const objects = (src.objects || [])
    .filter((o) => o.readable !== false)
    .map((so) => {
      const om = (spec.objects || []).find((m) => m.source_object === so.name);
      const rows = om ? om.fields.filter((fm) => fm.status !== "rejected").map((fm) => ({ ...exprToRow(fm.expr), target: fm.target, status: fm.status, confidence: fm.confidence, explanation: fm.explanation || "", evidence: fm.evidence || [] })) : [];
      const model = { source_object: so.name, include: !!om, target_object: om ? om.target_object : creates ? so.name : "", key: om ? om.target_key_field || null : null, rows };
      if (!creates) fillFixedTargets(model, dst);
      return model;
    });
  return { src, dst, creates, objects, current: (objects.find((o) => o.include) || objects[0] || {}).source_object, samples: {}, sourceName: resp.source.connection, destName: resp.destination.connection, updates: resp.destination_supports_updates };
}

/** For destinations with a fixed schema, show every writable field so nothing required is missed. */
function fillFixedTargets(model, dst) {
  const to = (dst.objects || []).find((o) => o.name === model.target_object);
  if (!to) return;
  for (const f of to.fields) {
    if (f.read_only || model.rows.some((r) => r.target === f.name)) continue;
    model.rows.push({ kind: "copy", source: null, params: {}, target: f.name, status: "accepted", confidence: 1, explanation: "", evidence: [] });
  }
  const order = to.fields.map((f) => f.name);
  model.rows.sort((a, b) => order.indexOf(a.target) - order.indexOf(b.target));
}

async function renderMap(alive) {
  const w = S.wiz;
  const key = `${w.source}>${w.destination}`;
  if (!w.map || w.mapKey !== key) {
    mount(...stepper(2), h("div", { class: "row dim" }, h("span", { class: "spinner" }), "Reading both systems and matching fields…"));
    let resp;
    try {
      resp = await api("POST", "/mapping/suggest", { source: w.source, destination: w.destination });
    } catch (e) {
      if (!alive()) return;
      mount(...stepper(2), notice("bad", h("strong", {}, "Could not read the systems. "), e.message), h("div", { style: "margin-top:16px" }, h("button", { class: "btn", type: "button", onclick: () => ((w.step = 1), route()) }, icon("left"), "Back to connections")));
      return;
    }
    if (!alive()) return;
    w.map = buildMapModel(resp, w.pendingMapping);
    w.pendingMapping = null;
    w.mapKey = key;
  }
  drawMap(alive);
  const onResize = () => drawWires();
  window.addEventListener("resize", onResize);
  return () => window.removeEventListener("resize", onResize);
}

function mapStats(m) {
  let mapped = 0,
    review = 0,
    unused = 0,
    required = 0;
  for (const o of m.objects.filter((x) => x.include)) {
    const so = m.src.objects.find((x) => x.name === o.source_object);
    const used = new Set(o.rows.filter((r) => r.source).map((r) => r.source.split(".")[0]));
    mapped += o.rows.filter((r) => r.source || r.kind === "const" || r.kind === "advanced").length;
    review += o.rows.filter((r) => r.status === "needs_review").length;
    unused += so.fields.filter((f) => !used.has(f.name)).length;
    required += missingRequired(m, o).length;
  }
  return { mapped, review, unused, required };
}

/** Destination fields that must have a value but have nothing mapped to them yet. */
function missingRequired(m, o) {
  const to = m.dst.objects.find((x) => x.name === o.target_object);
  if (!to) return [];
  return to.fields.filter((f) => f.required && !f.read_only && !o.rows.some((r) => r.target === f.name && (r.source || r.kind === "const" || r.kind === "advanced"))).map((f) => f.name);
}

function drawMap(alive) {
  const w = S.wiz;
  const m = w.map;
  const redraw = () => alive() && drawMap(alive);
  const cur = m.objects.find((o) => o.source_object === m.current) || m.objects[0];
  if (!cur) {
    mount(...stepper(2), notice("warn", "The source has no data sets to migrate."), h("div", { style: "margin-top:16px" }, h("button", { class: "btn", type: "button", onclick: () => ((w.step = 1), route()) }, icon("left"), "Back to connections")));
    return;
  }
  const so = m.src.objects.find((x) => x.name === cur.source_object);
  const stats = mapStats(m);

  const objectList = h(
    "nav",
    { class: "panel tight object-list", "aria-label": "Data sets" },
    h("div", { class: "upper dim", style: "margin-bottom:8px" }, "Data sets"),
    m.objects.map((o) => {
      const rev = o.rows.filter((r) => r.status === "needs_review").length;
      return h(
        "button",
        { type: "button", "aria-current": String(o === cur), onclick: () => ((m.current = o.source_object), redraw()) },
        o.include ? (rev ? icon("alert") : icon("check")) : icon("x"),
        h("span", { class: "grow" }, h("div", { class: o.include ? "" : "off" }, o.source_object), h("div", { class: "sub" }, o.include ? `→ ${o.target_object || "?"}${rev ? ` · ${rev} to check` : ""}` : "not included")),
      );
    }),
  );

  const curGuesses = cur.rows.filter((r) => r.status === "needs_review");
  const acceptRow = (r) => ((r.status = "accepted"), (r.confidence = 1), (r.explanation = "accepted by you"));
  /** Open the first data set that still has the given kind of problem. */
  const jumpTo = (pred) => () => {
    const o = m.objects.find((x) => x.include && pred(x));
    if (o) ((m.current = o.source_object), redraw());
  };

  // header: include + target
  const include = h("input", { type: "checkbox", checked: cur.include, onchange: (e) => includeObject(cur, e.target.checked, redraw) });
  let targetCtl;
  if (m.creates) {
    targetCtl = h("input", { type: "text", value: cur.target_object, "aria-label": "Name in the destination", onchange: (e) => ((cur.target_object = e.target.value.trim()), redraw()) });
  } else {
    targetCtl = h(
      "select",
      { "aria-label": "Destination data set", onchange: (e) => retarget(cur, e.target.value, redraw) },
      h("option", { value: "" }, "Choose…"),
      m.dst.objects.filter((o) => o.writable !== false).map((o) => h("option", { value: o.name, selected: o.name === cur.target_object }, o.name)),
    );
  }
  const head = h(
    "div",
    { class: "panel tight stack" },
    h("div", { class: "row" }, h("label", { class: "check" }, include, h("strong", {}, `Migrate "${cur.source_object}"`)), h("span", { class: "dim" }, "into"), h("div", { style: "flex:1 1 220px;max-width:340px" }, targetCtl)),
    cur.include
      ? cur.key
        ? notice("ok", h("strong", {}, `Records are matched by "${cur.key}". `), "Running the migration again will update the same records instead of creating duplicates.")
        : notice("warn", h("strong", {}, "No ID field is marked. "), "Click the key button on the field that holds each record's ID. Without it, re-running can create duplicates and verification can only count.")
      : notice("info", "This data set is not part of the migration. Tick the box to include it."),
    cur.include && curGuesses.length
      ? h(
          "div",
          { class: "row" },
          h("span", { class: "dim grow" }, `${plural(curGuesses.length, "yellow row")} below ${curGuesses.length === 1 ? "is a guess" : "are guesses"}. Check ${curGuesses.length === 1 ? "it" : "them"}, then accept one by one or all at once.`),
          h("button", { class: "btn small", type: "button", onclick: () => (curGuesses.forEach(acceptRow), redraw()) }, icon("check"), curGuesses.length === 1 ? "Accept the guess" : `Accept all ${curGuesses.length} guesses here`),
        )
      : null,
  );

  // board
  const board = h("div", { class: "board" });
  const used = new Set(cur.rows.filter((r) => r.source).map((r) => r.source.split(".")[0]));
  const sample = m.samples[cur.source_object];
  const srcCol = h(
    "div",
    { class: "src-col" },
    h("div", { class: "col-head" }, `From · ${cur.source_object}`),
    so.fields.map((f) => {
      const eg = sample ? sample[f.name] : undefined;
      return h(
        "div",
        { class: `src-field ${used.has(f.name) ? "" : "unused"}`, "data-field": f.name, title: used.has(f.name) ? "" : "Not mapped: its values are kept in the archive, not lost." },
        h("div", { class: "row", style: "gap:8px" }, h("strong", {}, f.name), h("span", { class: "faint" }, typeName(f.field_type)), used.has(f.name) ? null : h("span", { class: "badge", style: "margin-left:auto" }, "archived")),
        eg !== undefined ? h("div", { class: "eg" }, `e.g. ${eg === null ? "(empty)" : typeof eg === "object" ? JSON.stringify(eg) : String(eg)}`) : null,
      );
    }),
  );
  const tgtCol = h("div", { class: "tgt-col" }, h("div", { class: "col-head" }, `To · ${cur.target_object || "?"}`));
  if (cur.include) {
    cur.rows.forEach((row, idx) => tgtCol.append(targetRow(m, cur, so, row, idx, redraw)));
    if (m.creates) {
      tgtCol.append(h("button", { class: "btn small", type: "button", onclick: () => (cur.rows.push({ kind: "copy", source: null, params: {}, target: "", status: "accepted", confidence: 1, explanation: "", evidence: [] }), redraw()) }, icon("plus"), "Add a field"));
    }
  } else {
    tgtCol.append(h("div", { class: "empty" }, icon("x"), "Not included."));
  }
  const wires = document.createElementNS(SVG_NS, "svg");
  wires.setAttribute("class", "wires");
  wires.setAttribute("aria-hidden", "true");
  board.append(srcCol, h("div", { class: "gutter" }), tgtCol, wires);

  const create = h("button", { class: "btn primary big", type: "button", onclick: () => busy(create, () => createPlan()) }, "Create the plan", icon("right"));
  mount(
    ...stepper(2),
    h("h1", {}, "Which field goes where?"),
    h("p", { class: "dim", style: "margin:8px 0 16px" }, "The lines show what was matched for you. Change anything with the menus. Yellow rows are guesses: accept or correct them."),
    w.demo ? h("div", { style: "margin-bottom:16px" }, notice("info", h("strong", {}, "Demo data. "), `${fmt(w.demo.records)} made-up CRM records will be copied into new files in the folder `, h("span", { class: "mono", title: w.demo.output_path }, w.demo.output_path.split(/[\\/]/).slice(-2).join("/")), ". Nothing leaves this computer.")) : null,
    h("div", { class: "map-layout" }, objectList, h("div", { class: "stack" }, head, h("div", { class: "panel tight", style: "overflow-x:auto" }, board))),
    h(
      "div",
      { class: "summary-bar" },
      badge("ok", `${fmt(stats.mapped)} mapped`, "check"),
      stats.review ? h("button", { class: "badge warn as-button", type: "button", title: "Show the first guess", onclick: jumpTo((o) => o.rows.some((r) => r.status === "needs_review")) }, icon("alert"), `${fmt(stats.review)} to check`) : null,
      stats.required ? h("button", { class: "badge bad as-button", type: "button", title: "Show the first empty required field", onclick: jumpTo((o) => missingRequired(m, o).length > 0) }, icon("alert"), `${plural(stats.required, "required field")} empty`) : null,
      stats.unused ? badge("info", `${fmt(stats.unused)} kept in archive`, "file") : null,
      h("span", { class: "grow" }),
      h("button", { class: "btn ghost", type: "button", onclick: () => ((w.step = 1), route()) }, icon("left"), "Back"),
      create,
    ),
  );
  requestAnimationFrame(drawWires);
  if (sample === undefined) {
    m.samples[cur.source_object] = null;
    api("POST", `/connections/${encodeURIComponent(m.sourceName)}/sample`, { object: cur.source_object, limit: 1 })
      .then((r) => {
        m.samples[cur.source_object] = (r.records || [])[0] || null;
        if (m.samples[cur.source_object]) redraw();
      })
      .catch(() => {});
  }
}

const typeName = (t) => ({ string: "text", int: "number", float: "number", decimal: "decimal", bool: "yes/no", date: "date", date_time: "date+time", array: "list", object: "group", bytes: "binary", unknown: "" })[t] ?? t;

function targetRow(m, cur, so, row, idx, redraw) {
  const to = m.dst.objects.find((x) => x.name === cur.target_object);
  const def = to && to.fields.find((f) => f.name === row.target);
  const touched = () => {
    row.status = "accepted";
    row.confidence = 1;
    row.explanation = "set by you";
  };
  const active = !!(row.source || row.kind === "const" || row.kind === "advanced");
  const nameCtl = m.creates
    ? h("input", { type: "text", value: row.target, placeholder: "field name", "aria-label": "Destination field name", onchange: (e) => ((row.target = e.target.value.trim()), redraw()) })
    : h("div", { class: "tname" }, h("span", { class: "t" }, h("strong", {}, row.target)), h("span", { class: "faint" }, [typeName(def ? def.field_type : ""), def && def.required ? " · required" : ""].join("")));
  const from = h(
    "select",
    { "aria-label": `Source for ${row.target || "field"}`, onchange: (e) => ((row.source = e.target.value || null), touched(), redraw()) },
    h("option", { value: "" }, row.kind === "const" ? "(not needed)" : "— not mapped —"),
    so.fields.map((f) => h("option", { value: f.name, selected: (row.source || "").split(".")[0] === f.name }, f.name)),
  );
  const rule = h(
    "select",
    {
      "aria-label": `Rule for ${row.target || "field"}`,
      onchange: (e) => {
        row.kind = e.target.value;
        if (row.kind === "advanced") row.params = { json: JSON.stringify(row.source ? { op: "field", path: row.source } : { op: "const", value: { type: "string", value: "" } }, null, 1) };
        else if (row.kind === "cast") row.params = { to: "string" };
        else if (row.kind === "enum") row.params = { pairs: "", on_unmapped: "fail" };
        else row.params = {};
        touched();
        redraw();
      },
    },
    TRANSFORMS.map(([v, l]) => h("option", { value: v, selected: row.kind === v }, l)),
  );
  const isKey = cur.key && cur.key === row.target;
  const actions = h(
    "div",
    { class: "actions" },
    row.status === "needs_review" ? h("button", { class: "btn small", type: "button", title: row.explanation, onclick: () => (touched(), redraw()) }, icon("check"), "Accept") : null,
    h("button", { class: "icon-btn", type: "button", "aria-pressed": String(!!isKey), "aria-label": `Use ${row.target} as the record ID`, title: "This field holds each record's ID", onclick: () => ((cur.key = isKey ? null : row.target), redraw()) }, icon("key")),
    m.creates ? h("button", { class: "icon-btn", type: "button", "aria-label": `Remove ${row.target}`, title: "Remove this field", onclick: () => (cur.rows.splice(idx, 1), cur.key === row.target && (cur.key = null), redraw()) }, icon("trash")) : null,
  );
  const missing = !!(def && def.required && !def.read_only && !active);
  const el = h("div", { class: `tgt-row ${row.status === "needs_review" ? "review" : ""} ${missing ? "missing" : active ? "" : "off"}`, "data-row": String(idx) }, nameCtl, from, rule, actions);

  // rule parameters
  const p = row.params;
  const extra = h("div", { class: "extra" });
  const set = (k) => (e) => ((p[k] = e.target.type === "checkbox" ? e.target.checked : e.target.value), touched());
  if (row.kind === "cast") extra.append(h("span", { class: "dim" }, "convert to"), h("select", { "aria-label": "Target type", onchange: set("to") }, TYPES.map(([v, l]) => h("option", { value: v, selected: p.to === v }, l))));
  if (row.kind === "ref")
    extra.append(
      h("span", { class: "dim" }, "points to a record in"),
      h("select", { "aria-label": "Linked data set", onchange: set("object") }, h("option", { value: "" }, "Choose…"), m.objects.filter((o) => o.include).map((o) => h("option", { value: o.source_object, selected: p.object === o.source_object }, o.source_object))),
      h("span", { class: "faint" }, "its new ID is filled in automatically"),
    );
  if (row.kind === "scale") extra.append(h("span", { class: "dim" }, "move the decimal point"), h("input", { type: "number", value: p.pow ?? 2, "aria-label": "Places", style: "max-width:110px", onchange: set("pow") }), h("span", { class: "faint" }, "places (2 turns 12.50 into 1250)"));
  if (row.kind === "parse_date") extra.append(h("span", { class: "dim" }, "pattern"), h("input", { type: "text", value: p.format || "", placeholder: "%d.%m.%Y", "aria-label": "Date pattern", onchange: set("format") }), h("label", { class: "row", style: "gap:8px" }, h("input", { type: "checkbox", checked: !!p.datetime, onchange: set("datetime") }), "includes a time"));
  if (row.kind === "enum")
    extra.append(
      h("textarea", { "aria-label": "Translations, one per line", placeholder: "A=active\nI=inactive", onchange: set("pairs") }, p.pairs || ""),
      h("select", { "aria-label": "When a value is not listed", onchange: set("on_unmapped") }, [["fail", "If not listed: stop that record"], ["keep", "If not listed: keep the value"], ["null_and_archive", "If not listed: leave empty, archive it"]].map(([v, l]) => h("option", { value: v, selected: p.on_unmapped === v }, l))),
    );
  if (row.kind === "const") extra.append(h("span", { class: "dim" }, "value"), h("input", { type: "text", value: p.value || "", "aria-label": "Fixed value", onchange: set("value") }));
  if (row.kind === "advanced") extra.append(h("textarea", { "aria-label": "Rule as JSON", spellcheck: "false", onchange: set("json") }, p.json || ""));
  if (row.status === "needs_review" && row.explanation) extra.append(h("div", { class: "row-hint warn" }, icon("alert"), h("span", {}, `Guess (${Math.round((row.confidence || 0) * 100)}% sure): ${row.explanation.replaceAll("`", '"')}`)));
  if (missing) extra.append(h("div", { class: "row-hint bad" }, icon("alert"), h("span", {}, "The destination needs a value here. Choose where it comes from, or pick the rule \"Always the same value\".")));
  if (extra.childNodes.length) el.append(extra);

  const hot = (on) => () => {
    el.classList.toggle("hot", on);
    const s = row.source && document.querySelector(`.src-field[data-field="${CSS.escape(row.source.split(".")[0])}"]`);
    s && s.classList.toggle("hot", on);
    const wire = document.querySelector(`.wire[data-row="${idx}"]`);
    wire && wire.classList.toggle("hot", on);
  };
  el.addEventListener("mouseenter", hot(true));
  el.addEventListener("mouseleave", hot(false));
  el.addEventListener("focusin", hot(true));
  el.addEventListener("focusout", hot(false));
  return el;
}

function drawWires() {
  const board = document.querySelector(".board");
  const svg = board && board.querySelector("svg.wires");
  if (!svg) return;
  svg.replaceChildren();
  const b = board.getBoundingClientRect();
  const m = S.wiz.map;
  const cur = m && m.objects.find((o) => o.source_object === m.current);
  if (!cur || !cur.include) return;
  cur.rows.forEach((row, idx) => {
    if (!row.source) return;
    const s = board.querySelector(`.src-field[data-field="${CSS.escape(row.source.split(".")[0])}"]`);
    const t = board.querySelector(`.tgt-row[data-row="${idx}"]`);
    if (!s || !t) return;
    const sr = s.getBoundingClientRect();
    const tr = t.getBoundingClientRect();
    const x1 = sr.right - b.left,
      y1 = sr.top + sr.height / 2 - b.top,
      x2 = tr.left - b.left,
      y2 = tr.top + Math.min(tr.height / 2, 26) - b.top;
    const mid = (x1 + x2) / 2;
    const path = document.createElementNS(SVG_NS, "path");
    path.setAttribute("d", `M${x1} ${y1} C${mid} ${y1}, ${mid} ${y2}, ${x2} ${y2}`);
    path.setAttribute("class", `wire ${row.status === "needs_review" ? "review" : ""}`);
    path.setAttribute("data-row", String(idx));
    svg.append(path);
  });
}

function includeObject(cur, on, redraw) {
  const m = S.wiz.map;
  cur.include = on;
  if (on && !cur.rows.length) {
    const so = m.src.objects.find((x) => x.name === cur.source_object);
    if (m.creates) {
      cur.target_object = cur.target_object || cur.source_object;
      cur.rows = so.fields.map((f) => ({ kind: "copy", source: f.name, params: {}, target: f.name, status: "accepted", confidence: 1, explanation: "copied 1:1", evidence: [] }));
      cur.key = (so.primary_key || [])[0] || null;
    } else if (cur.target_object) {
      retarget(cur, cur.target_object, redraw);
      return;
    }
  }
  redraw();
}

/** Destination data set changed: match fields with the same name, list the rest unmapped. */
function retarget(cur, target, redraw) {
  const m = S.wiz.map;
  cur.target_object = target;
  cur.rows = [];
  cur.key = null;
  const so = m.src.objects.find((x) => x.name === cur.source_object);
  const to = m.dst.objects.find((x) => x.name === target);
  if (to) {
    const norm = (s) => s.toLowerCase().replace(/[^a-z0-9]/g, "");
    for (const f of to.fields.filter((x) => !x.read_only)) {
      const hit = so.fields.find((s) => norm(s.name) === norm(f.name));
      cur.rows.push({ kind: "copy", source: hit ? hit.name : null, params: {}, target: f.name, status: "accepted", confidence: 1, explanation: hit ? "same name" : "", evidence: [] });
    }
    cur.include = true;
  }
  redraw();
}

function buildMappingSpec() {
  const m = S.wiz.map;
  const objects = [];
  for (const o of m.objects.filter((x) => x.include)) {
    if (!o.target_object) throw new Error(`Choose where "${o.source_object}" should go, or untick it.`);
    const fields = [];
    const seen = new Set();
    for (const row of o.rows) {
      const active = row.source || row.kind === "const" || row.kind === "advanced";
      if (!active) continue;
      if (!row.target) throw new Error(`"${o.source_object}": a destination field has no name.`);
      if (seen.has(row.target)) throw new Error(`"${o.source_object}": the destination field "${row.target}" appears twice.`);
      seen.add(row.target);
      fields.push({ target: row.target, expr: rowToExpr(row), confidence: row.confidence ?? 1, status: row.status || "accepted", explanation: row.explanation || "", evidence: row.evidence || [], deferred: false });
    }
    if (!fields.length) throw new Error(`"${o.source_object}" has no mapped fields. Map at least one or untick it.`);
    const key = o.key && seen.has(o.key) ? o.key : null;
    const spec = { source_object: o.source_object, target_object: o.target_object, fields, unmapped_fields: "archive", skip_fields: [], write_mode: key ? "upsert" : "insert" };
    if (key) spec.target_key_field = key;
    objects.push(spec);
  }
  if (!objects.length) throw new Error("Include at least one data set.");
  return { mapping_version: 1, objects };
}

async function createPlan() {
  const w = S.wiz;
  const mapping = buildMappingSpec();
  const summary = await api("POST", "/plans", { source: w.source, destination: w.destination, mapping });
  await api("POST", `/plans/${summary.plan_id}/dry-run`, { sample: 200 }).catch((e) => toast(`The test run could not finish: ${e.message}`, "warn"));
  go(`#/plan/${summary.plan_id}`);
}

// ============================================================ steps 3 & 4: plan and approval
async function viewPlan(alive, id, approveRoute) {
  await loadInfo();
  mount(...stepper(3), h("div", { class: "row dim" }, h("span", { class: "spinner" }), "Loading the plan…"));
  let data = await api("GET", `/plans/${id}`);
  if (!data.dry_run) {
    await api("POST", `/plans/${id}/dry-run`, { sample: 200 }).catch(() => {});
    data = await api("GET", `/plans/${id}`);
  }
  if (!alive()) return;
  // #/plan/<id>/approve opens straight on the approval step
  const state = { stage: data.approval.state === "approved" || (approveRoute && data.summary.approvable) ? 4 : 3 };
  const render = () => alive() && drawPlan(id, data, state, render, alive);
  render();
}

function riskLists(plan) {
  const by = (sev) => (plan.risks || []).filter((r) => r.severity === sev);
  // general limits of a connector are background reading, not a problem with this plan
  const general = (r) => /_limitation$/.test(r.code || "");
  return { blocking: by("blocking"), warnings: by("warning").filter((r) => !general(r)), technical: by("warning").filter(general), info: by("info") };
}

function drawPlan(id, data, state, render, alive) {
  const { plan, summary, dry_run: dry, approval } = data;
  const risks = riskLists(plan);
  const total = Object.values(plan.estimated_records || {}).every((n) => n != null) ? Object.values(plan.estimated_records).reduce((a, b) => a + b, 0) : null;
  const approved = approval.state === "approved";
  const destructive = (plan.operations || []).includes("truncate_destination");

  const editMapping = () => {
    S.wiz = freshWizard();
    S.wiz.source = plan.source.connection;
    S.wiz.destination = plan.destination.connection;
    S.wiz.pendingMapping = plan.mapping;
    S.wiz.step = 2;
    go("#/new");
  };

  const overview = h(
    "section",
    { class: "panel stack" },
    h("div", { class: "panel-title" }, h("h2", {}, icon("list"), "What will happen"), h("span", { class: "right faint mono" }, `${plan.plan_id} · ${summary.plan_hash.slice(0, 12)}`)),
    h(
      "div",
      { class: "flow" },
      h("div", { class: "node" }, h("div", { class: "upper dim" }, "From"), h("strong", {}, plan.source.connection), h("div", { class: "faint" }, `${plan.source.identity.name} · ${kindLabel(plan.source.identity.kind)}`)),
      icon("right"),
      h("div", { class: "node" }, h("div", { class: "upper dim" }, "To"), h("strong", {}, plan.destination.connection), h("div", { class: "faint" }, `${plan.destination.identity.name} · ${kindLabel(plan.destination.identity.kind)}`), badge(plan.destination.environment === "production" ? "warn" : "ok", plan.destination.environment)),
      h("div", { class: "node" }, h("div", { class: "upper dim" }, "Records"), h("strong", {}, total == null ? "counted while running" : fmt(total))),
    ),
    h(
      "div",
      { class: "table-wrap" },
      h(
        "table",
        {},
        h("thead", {}, h("tr", {}, h("th", {}, "Order"), h("th", {}, "From"), h("th", {}, "To"), h("th", { class: "num" }, "Records"), h("th", {}, "How"))),
        h(
          "tbody",
          {},
          plan.execution_order.map((name, i) => {
            const om = plan.mapping.objects.find((o) => o.source_object === name);
            return h("tr", {}, h("td", {}, i + 1), h("td", {}, name), h("td", {}, om.target_object), h("td", { class: "num" }, fmt(plan.estimated_records[name])), h("td", { class: "dim" }, om.target_key_field ? `matched by ${om.target_key_field} (safe to re-run)` : "added as new records"));
          }),
        ),
      ),
    ),
    plan.deferred_references && plan.deferred_references.length ? notice("info", `Some records point at each other (${plan.deferred_references.map((d) => `${d.source_object}.${d.target_field}`).join(", ")}). Those links are filled in during a second pass.`) : null,
  );

  const issues = h("section", { class: "panel stack" }, h("div", { class: "panel-title" }, h("h2", {}, icon("alert"), "Things to know")));
  if (risks.blocking.length) issues.append(notice("bad", h("strong", {}, "This must be fixed before you can approve:"), h("ul", {}, risks.blocking.map((r) => h("li", {}, r.object ? h("strong", {}, `${r.object}: `) : null, r.message))), h("div", { style: "margin-top:12px" }, h("button", { class: "btn small", type: "button", onclick: editMapping }, icon("left"), "Fix it in the mapping"))));
  if (risks.warnings.length) issues.append(notice("warn", h("strong", {}, "Worth a look:"), h("ul", {}, risks.warnings.map((r) => h("li", {}, r.object ? h("strong", {}, `${r.object}: `) : null, r.message)))));
  if (risks.info.length) issues.append(h("details", {}, h("summary", {}, `${plural(risks.info.length, "note")} about fields that are not mapped`), h("div", { class: "body" }, h("ul", {}, risks.info.map((r) => h("li", {}, r.object ? h("strong", {}, `${r.object}: `) : null, r.message))))));
  if (!risks.blocking.length && !risks.warnings.length) issues.append(notice("ok", "No problems found in the plan."));
  if (risks.technical.length) issues.append(h("details", {}, h("summary", {}, `${plural(risks.technical.length, "technical note")} about ${kindLabel(plan.destination.identity.kind)} destinations`), h("div", { class: "body" }, h("ul", {}, risks.technical.map((r) => h("li", {}, r.message))))));

  const dryPanel = h("section", { class: "panel stack" }, h("div", { class: "panel-title" }, h("h2", {}, icon("play"), "Test run"), h("span", { class: "right" }, badge("ok", "nothing was written", "shield"))));
  if (dry) {
    dryPanel.append(
      notice(dry.passed ? "ok" : "warn", h("strong", {}, dry.passed ? "The test run passed. " : "The test run found problems. "), "A sample of real records was read and transformed without writing anything."),
      h(
        "div",
        { class: "table-wrap" },
        h(
          "table",
          {},
          h("thead", {}, h("tr", {}, h("th", {}, "Data set"), h("th", { class: "num" }, "Sampled"), h("th", { class: "num" }, "Would succeed"), h("th", { class: "num" }, "Would fail"), h("th", {}, "Kept in archive"))),
          h("tbody", {}, dry.objects.map((o) => h("tr", {}, h("td", {}, `${o.source_object} → ${o.target_object}`), h("td", { class: "num" }, fmt(o.sampled)), h("td", { class: "num" }, fmt(o.transform_ok)), h("td", { class: "num" }, o.transform_failed ? h("span", { class: "badge bad" }, fmt(o.transform_failed)) : "0"), h("td", { class: "dim" }, (o.archived_fields || []).join(", ") || "—")))),
        ),
      ),
    );
    const examples = dry.objects.flatMap((o) => o.examples || []);
    if (examples.length) dryPanel.append(h("details", { open: true }, h("summary", {}, `Why records would fail (${examples.length} example${examples.length === 1 ? "" : "s"})`), h("div", { class: "body" }, h("ul", {}, examples.map((x) => h("li", {}, x))))));
  } else {
    dryPanel.append(notice("warn", "The test run has not finished. You can try it again."));
  }
  const rerun = h("button", { class: "btn small", type: "button", onclick: () => busy(rerun, async () => ((data.dry_run = await api("POST", `/plans/${id}/dry-run`, { sample: 200 })), render())) }, icon("refresh"), "Run the test again");
  dryPanel.append(rerun);

  const nodes = [...stepper(state.stage)];
  if (state.stage === 3) {
    const next = h("button", { class: "btn primary big", type: "button", disabled: !summary.approvable, onclick: () => ((state.stage = 4), render(), window.scrollTo(0, 0)) }, "Continue to approval", icon("right"));
    nodes.push(
      h("h1", {}, "Here is the plan."),
      h("p", { class: "dim", style: "margin:8px 0 24px" }, "Nothing has been written. Read it through; you approve in the next step."),
      h("div", { class: "stack-lg" }, overview, issues, dryPanel),
      h("div", { class: "summary-bar" }, h("span", { class: "dim grow" }, summary.approvable ? "The plan is ready for approval." : "The plan has issues that must be fixed first."), h("button", { class: "btn ghost", type: "button", onclick: editMapping }, icon("left"), "Change the mapping"), next),
    );
  } else {
    nodes.push(h("h1", {}, approved ? "Approved. Ready when you are." : "Your approval is needed."), h("p", { class: "dim", style: "margin:8px 0 24px" }, approved ? friendlyApproval(approval.detail) : "Only a person can approve a migration. Migrate Anything never approves on its own, and neither can an AI assistant."));
    const panel = h("section", { class: "panel stack" });
    const summaryList = h(
      "dl",
      { class: "kv" },
      h("dt", {}, "From"),
      h("dd", {}, `${plan.source.connection} (${plan.source.identity.name})`),
      h("dt", {}, "To"),
      h("dd", {}, `${plan.destination.connection} (${plan.destination.identity.name}) — ${plan.destination.environment}`),
      h("dt", {}, "Writes"),
      h("dd", {}, `${total == null ? "all" : fmt(total)} records into ${plan.execution_order.length} data set(s)`),
      h("dt", {}, "Plan"),
      h("dd", { class: "mono" }, `${plan.plan_id} · sha256 ${summary.plan_hash.slice(0, 16)}…`),
    );
    panel.append(h("div", { class: "panel-title" }, h("h2", {}, icon("shield"), approved ? "Approved" : "Approve this plan")), summaryList);
    if (approved) {
      // Re-approving a plan whose migration stopped part-way: lead back to that migration
      // instead of offering to start a second one from the first record.
      const stopped = S.resume && S.resume.plan === id ? S.resume.job : null;
      const start = stopped
        ? h("a", { class: "btn primary big", href: `#/job/${stopped}`, onclick: () => (S.resume = null) }, icon("play"), "Back to the stopped migration")
        : h("button", { class: "btn primary big", type: "button", onclick: () => busy(start, async () => go(`#/job/${(await api("POST", "/jobs", { plan_id: id })).job_id}`)) }, icon("play"), "Start the migration");
      if (stopped) panel.append(notice("info", "This plan already has a migration that stopped part-way. Go back to it and continue from where it stopped."));
      panel.append(h("div", { class: "row" }, start, h("button", { class: "btn ghost", type: "button", onclick: () => ((state.stage = 3), render()) }, icon("left"), "Back to the plan")));
    } else if (!summary.approvable) {
      panel.append(notice("bad", "This plan has issues that must be fixed before it can be approved."), h("button", { class: "btn", type: "button", onclick: () => ((state.stage = 3), render()) }, icon("left"), "Back to the plan"));
    } else if (!S.info.approvals.enabled) {
      const check = h("button", { class: "btn primary", type: "button", onclick: () => busy(check, async () => ((data.approval = (await api("GET", `/plans/${id}`)).approval), data.approval.state !== "approved" && toast("Not approved yet.", "warn"), render())) }, icon("refresh"), "I have approved it — check");
      panel.append(
        notice("warn", h("strong", {}, "Approve from a terminal. "), "This Studio was started without an interactive terminal, so it cannot take your approval here. Run this command yourself:"),
        h("div", { class: "cmd" }, h("code", {}, approval.operator_command), copyButton(approval.operator_command)),
        check,
      );
    } else {
      const allow = h("input", { type: "checkbox" });
      const extra = destructive ? h("label", { class: "check" }, allow, h("span", {}, h("strong", {}, "I allow existing destination data to be replaced."), h("div", { class: "help" }, "This plan overwrites files that already exist in the destination."))) : null;
      panel.append(
        codeFlow({
          kind: "plan",
          subject: id,
          label: "Approve the migration",
          extra,
          submit: (challenge_id, code) => api("POST", `/plans/${id}/approve`, { challenge_id, code, allow_destructive: !!(destructive && allow.checked) }),
          done: async () => {
            data.approval = (await api("GET", `/plans/${id}`)).approval;
            toast("Approved.");
            render();
          },
        }),
        h("button", { class: "btn ghost", type: "button", onclick: () => ((state.stage = 3), render()) }, icon("left"), "Back to the plan"),
      );
    }
    nodes.push(panel);
  }
  mount(...nodes);
}

/** "approved by ann (method) until 2026-…T…" → a sentence with a local date. */
function friendlyApproval(detail) {
  const m = /^approved by (\S+) \(([^)]*)\) until (\S+)$/.exec(detail || "");
  return m ? `Approved by ${m[1]}. Valid until ${when(m[3])}.` : detail;
}

function copyButton(text) {
  const b = h("button", { class: "btn small", type: "button", onclick: async () => (await navigator.clipboard.writeText(text).catch(() => {}), toast("Copied.")) }, icon("copy"), "Copy");
  return b;
}

// ============================================================ step 5: run & proof
const WRITTEN = ["migrated_natively", "transformed", "written_with_archive"];
const stageWritten = (s) => WRITTEN.reduce((a, k) => a + ((s.counters.outcomes || {})[k] || 0), 0);
const stageFailed = (s) => ((s.counters.outcomes || {}).failed || 0) + ((s.counters.outcomes || {}).unsupported || 0);
const TERMINAL = ["completed", "failed", "cancelled"];

async function viewJob(alive, id, replayRoute) {
  await loadInfo();
  mount(...stepper(5), h("div", { class: "row dim" }, h("span", { class: "spinner" }), "Loading…"));
  let job = await api("GET", `/jobs/${id}`);
  const planData = await api("GET", `/plans/${job.plan_id}`);
  if (!alive()) return;
  const plan = planData.plan;
  const estimates = plan.estimated_records || {};
  const total = plan.execution_order.every((o) => estimates[o] != null) ? plan.execution_order.reduce((a, o) => a + estimates[o], 0) : null;

  // ---- static frame
  const canvas = h("canvas", { "aria-hidden": "true" });
  const big = h("div", { class: "big" }, "0");
  const ofTotal = h("div", { class: "lbl" }, total == null ? "records migrated" : `of ${fmt(total)} records`);
  const rate = h("div", { class: "mid" }, "—");
  const elapsed = h("div", { class: "mid" }, "—");
  const statusText = h("div", { class: "mid" }, "…");
  const overall = h("div", { class: "progress", role: "progressbar", "aria-label": "Overall progress", "aria-valuemin": "0", "aria-valuemax": "100" }, h("i", {}));
  const stage = h(
    "div",
    { class: "stage" },
    canvas,
    h(
      "div",
      { class: "hud" },
      h("div", { class: "hud-top" }, h("div", { class: "hud-box" }, big, ofTotal), h("span", { style: "flex:1" }), h("div", { class: "hud-box" }, h("div", { class: "lbl" }, "Status"), statusText)),
      h("div", { class: "stack" }, h("div", { class: "hud-bottom" }, h("div", { class: "hud-box" }, h("div", { class: "lbl" }, "Speed"), rate), h("div", { class: "hud-box" }, h("div", { class: "lbl" }, "Time"), elapsed)), overall),
    ),
  );
  const controls = h("div", { class: "row" });
  const stagesBox = h("div", {});
  const live = h("p", { class: "dim", role: "status", "aria-live": "polite" });
  const proof = h("div", { class: "stack-lg" });
  const errorBox = h("div", {});

  mount(
    ...stepper(5),
    h("div", { class: "row", style: "margin-bottom:16px" }, h("h1", { class: "grow" }, `${plan.source.connection} `, h("span", { class: "glow" }, "→"), ` ${plan.destination.connection}`), controls),
    h("div", { class: "stack-lg" }, stage, live, errorBox, h("section", { class: "panel" }, h("div", { class: "panel-title" }, h("h2", {}, icon("list"), "Data sets")), stagesBox), proof),
  );
  const rain = new CodeRain(canvas, { fontSize: 18, fade: 0.07 });
  rain.setEnabled(S.motion);

  // ---- live state
  // `real` is what the job has actually done (polled). `shown` eases towards it, so a job that
  // finishes in a blink is still readable: the counter and the rain play it back over a few
  // seconds. Status, time and speed always describe the real job.
  const real = { read: 0, written: 0, failed: 0 };
  const shown = { read: 0, written: 0 };
  let lastRead = null,
    lastAt = null,
    speed = 0,
    peak = 1,
    verifying = false,
    proofDone = false,
    lastAnnounce = 0,
    lastStatus = null,
    replaying = false,
    settling = false,
    timer = null,
    raf = null,
    lastFrame = null;

  const act = (action, confirmText) => async (e) => {
    const btn = e.currentTarget;
    if (confirmText && !(await confirmDialog("Cancel this migration?", confirmText, "Yes, cancel it", true))) return;
    await busy(btn, async () => {
      job = await api("POST", `/jobs/${id}/${action}`);
      draw();
      if (!TERMINAL.includes(job.status) && !timer) timer = setInterval(poll, 250);
    });
  };

  const catchingUp = () => shown.written < real.written || shown.read < real.read;

  /** Play a finished run back from zero: the same picture as the live run, a few seconds long. */
  const replay = () => {
    Object.assign(shown, { read: 0, written: 0 });
    replaying = true;
    draw();
    stage.scrollIntoView({ block: "center", behavior: S.motion ? "smooth" : "auto" });
  };

  /** Runs every animation frame: eases the displayed numbers and drives the rain. */
  function paint(t) {
    raf = alive() ? requestAnimationFrame(paint) : null;
    const dt = lastFrame == null ? 0 : Math.min(0.1, (t - lastFrame) / 1000);
    lastFrame = t;
    const scale = Math.max(total || 0, real.read, 1);
    for (const k of ["read", "written"]) {
      const gap = real[k] - shown[k];
      // live: close most of the gap quickly; replay: a steady five seconds from zero to done
      const stepBy = replaying ? (scale / 5) * dt : Math.max(gap * 3 * dt, (scale / 3.5) * dt);
      shown[k] = gap <= 0 ? real[k] : Math.min(real[k], shown[k] + stepBy);
    }
    const running = job.status === "running";
    const behind = catchingUp();
    if (behind) settling = true;
    else if (settling) {
      // the display has caught up with a finished run: offer "Watch it again"
      settling = false;
      replaying = false;
      if (TERMINAL.includes(job.status)) draw();
    }
    big.textContent = fmt(Math.floor(shown.written));
    const pct = total ? Math.min(100, (shown.read / total) * 100) : job.status === "completed" && !behind ? 100 : null;
    overall.classList.toggle("indeterminate", pct == null && (running || behind));
    overall.firstChild.style.width = pct == null ? "" : `${pct}%`;
    if (pct != null) overall.setAttribute("aria-valuenow", String(Math.round(pct)));
    const labels = { pending: "Starting…", running: "Migrating", paused: "Paused", completed: "Finished", failed: "Stopped", cancelled: "Cancelled" };
    statusText.textContent = replaying ? "Replay" : job.status === "completed" && behind ? "Finishing" : job.status === "completed" && verifying && !proofDone ? "Verifying" : labels[job.status] || job.status;
    // the rain is the data: heavier while records are flowing, calm when nothing moves
    const flow = running ? 0.35 + 0.65 * Math.min(1, speed / peak) : behind ? 0.95 : verifying && !proofDone ? 0.55 : job.status === "paused" ? 0.04 : 0.12;
    rain.setIntensity(flow);
    S.bgRain && S.bgRain.setIntensity(running || behind ? 0.55 : 0.25);
  }

  function draw() {
    const stages = job.stages.filter((s) => !s.source_object.endsWith("#pass2"));
    real.read = stages.reduce((a, s) => a + s.counters.read, 0);
    real.written = stages.reduce((a, s) => a + stageWritten(s), 0);
    real.failed = stages.reduce((a, s) => a + stageFailed(s), 0);
    const now = Date.now();
    if (lastAt != null && now > lastAt) {
      const inst = ((real.read - lastRead) * 1000) / (now - lastAt);
      speed = speed * 0.6 + Math.max(0, inst) * 0.4;
      peak = Math.max(peak, speed);
    }
    lastRead = real.read;
    lastAt = now;
    const running = job.status === "running";
    const started = job.started_at ? new Date(job.started_at).getTime() : null;
    const ended = job.finished_at && TERMINAL.includes(job.status) ? new Date(job.finished_at).getTime() : now;
    const took = started ? Math.max(0, ended - started) : null;
    elapsed.textContent = took == null ? "—" : took < 1000 ? "under 1s" : duration(took);
    rate.textContent = running ? `${fmt(Math.round(speed))} / second` : job.status === "completed" && took ? `${fmt(Math.round(real.read / Math.max(took / 1000, 0.5)))} / second average` : "—";

    const current = job.stages.find((s) => s.state === "running") || stages.find((s) => s.state !== "done") || stages[stages.length - 1];
    if (current) {
      const name = current.source_object.replace("#pass2", "");
      const om = plan.mapping.objects.find((o) => o.source_object === name);
      const so = (plan.source_schema.objects || []).find((o) => o.name === name);
      rain.setWords([...(so ? so.fields.map((f) => f.name) : []), ...(om ? om.fields.map((f) => f.target) : [])]);
    }

    stagesBox.replaceChildren(
      ...job.stages.map((s) => {
        const pass2 = s.source_object.endsWith("#pass2");
        const name = s.source_object.replace("#pass2", "");
        const om = plan.mapping.objects.find((o) => o.source_object === name);
        const est = estimates[name];
        const p = s.state === "done" ? 100 : est ? Math.min(100, (s.counters.read / est) * 100) : null;
        const ic = s.state === "done" ? "check" : s.state === "running" ? "play" : "clock";
        const f = stageFailed(s);
        return h(
          "div",
          { class: "stage-row" },
          h("div", { class: "name" }, icon(ic), h("span", {}, h("strong", {}, pass2 ? `${name} (linking records)` : name), h("div", { class: "faint" }, pass2 ? "second pass" : `→ ${om ? om.target_object : ""}`))),
          h("div", { class: `progress ${p == null && s.state === "running" ? "indeterminate" : ""}`, role: "progressbar", "aria-label": `${name} progress`, "aria-valuemin": "0", "aria-valuemax": "100", "aria-valuenow": p == null ? null : String(Math.round(p)) }, h("i", { style: p == null ? "" : `width:${p}%` })),
          h("div", { class: "counts" }, pass2 ? `${fmt(s.counters.attempted)} links` : `${fmt(stageWritten(s))}${est != null ? ` / ${fmt(est)}` : ""}`, f ? h("div", {}, badge("bad", `${fmt(f)} not written`, "alert")) : null),
        );
      }),
    );

    controls.replaceChildren(
      ...(job.status === "running" ? [h("button", { class: "btn", type: "button", onclick: act("pause") }, icon("pause"), "Pause")] : []),
      ...(job.status === "paused" || job.status === "failed" ? [h("button", { class: "btn primary", type: "button", onclick: act("resume") }, icon("play"), job.status === "failed" ? "Try again from where it stopped" : "Resume")] : []),
      ...(job.status === "completed" && !replaying && !catchingUp() ? [h("button", { class: "btn ghost", type: "button", title: "Play this run back from the start", onclick: replay }, icon("refresh"), "Watch it again")] : []),
      ...(["running", "paused", "pending"].includes(job.status) ? [h("button", { class: "btn danger", type: "button", onclick: act("cancel", "Records already written stay in the destination and are listed in the report. You can start a new migration later.") }, icon("stop"), "Cancel")] : []),
    );
    // An approval can run out (or be withdrawn) while a migration is paused. Nothing is written
    // without one; approving the same plan again lets this migration continue.
    const needsApproval = /Approve the plan again/.test(job.error || "");
    errorBox.replaceChildren(
      job.status === "failed" && job.error
        ? notice(
            "bad",
            h("strong", {}, "The migration stopped. "),
            needsApproval ? "Its approval is no longer valid: it ran out or was withdrawn. Approve the plan again, then come back here and continue." : job.error,
            needsApproval ? h("div", { style: "margin-top:10px" }, h("a", { class: "btn primary", href: `#/plan/${job.plan_id}/approve`, onclick: () => (S.resume = { plan: job.plan_id, job: job.job_id }) }, icon("shield"), "Approve the plan again")) : null,
            h("div", { class: "faint", style: "margin-top:6px" }, "Nothing is lost: it continues from the last saved point when you try again."),
          )
        : "",
    );

    // counts are announced every few seconds; a change of status is announced at once
    if (now - lastAnnounce > 5000 || job.status !== lastStatus) {
      lastAnnounce = now;
      lastStatus = job.status;
      live.textContent = running
        ? `${fmt(real.written)} records migrated${total ? ` of ${fmt(total)}` : ""}${real.failed ? `, ${fmt(real.failed)} not written` : ""}.`
        : job.status === "pending"
          ? "Waiting for the migration worker to start."
          : job.status === "paused"
            ? "Paused. Everything so far is saved."
            : "";
    }
    if (TERMINAL.includes(job.status) && !proofDone && !verifying) finish();
  }

  async function finish() {
    verifying = true;
    stopPolling();
    if (job.status === "completed") {
      const rep = await api("GET", `/jobs/${id}/report`).catch(() => null);
      if (!rep || !rep.report.verified) {
        proof.replaceChildren(h("section", { class: "panel" }, h("div", { class: "row" }, h("span", { class: "spinner" }), h("div", {}, h("strong", {}, "Checking every record…"), h("div", { class: "dim" }, "Reading the source and the destination again and comparing them.")))));
        try {
          await api("POST", `/jobs/${id}/verify`);
        } catch (e) {
          if (alive()) proof.replaceChildren(notice("bad", h("strong", {}, "The result could not be verified. "), e.message));
        }
      }
    }
    if (!alive()) return;
    await drawProof().catch((e) => proof.append(notice("bad", e.message)));
    proofDone = true;
    verifying = false;
  }

  async function drawProof() {
    const [rep, exc] = await Promise.all([api("GET", `/jobs/${id}/report`), api("GET", `/jobs/${id}/exceptions?limit=100`).catch(() => [])]);
    if (!alive()) return;
    const r = rep.report;
    const verdicts = {
      completed_and_verified: ["ok", "check", "Complete and verified", "Every record that was written was found in the destination, and its contents match."],
      completed_with_exceptions: ["warn", "alert", "Finished, with exceptions", "Some records or values need your attention. Nothing was dropped silently; the details are below."],
      partially_completed: ["warn", "alert", "Partly completed", "Not everything arrived. See the checks below, then try again from where it stopped."],
      failed: ["bad", "x", "Failed", "No records were written."],
      cancelled: ["warn", "stop", "Cancelled", "The migration was stopped. Records written before that are listed below."],
      not_verified: ["warn", "info", "Finished, not yet verified", "The records were written but the check has not run."],
    };
    const [kind, ic, title, text] = verdicts[r.final_status] || ["warn", "info", r.final_status, ""];
    const verification = r.verified ? await api("GET", `/jobs/${id}/verification`).catch(() => null) : null;
    const dest = plan.destination.identity;
    const where = dest.kind === "files" ? `the folder ${dest.locator.split("#")[0]}` : `${plan.destination.connection} (${dest.locator})`;
    const stats = [
      [r.records_discovered, "found in the source"],
      [r.records_written, "written"],
      [r.verified ? r.records_verified : null, "verified in the destination"],
      [r.records_failed, "not written"],
      [r.archived_values, "values kept in the archive"],
    ];
    if (r.relationships_checked) stats.push([`${fmt(r.relationships_preserved)} / ${fmt(r.relationships_checked)}`, "links between records intact"]);
    const nodes = [
      h(
        "section",
        { class: "panel stack" },
        h("div", { class: `verdict ${kind}` }, h("div", { class: "icon" }, icon(ic)), h("div", {}, h("h2", {}, title), h("p", {}, text))),
        h("div", { class: "stats" }, stats.map(([n, label]) => h("div", { class: "stat" }, h("b", {}, typeof n === "string" ? n : fmt(n)), h("span", {}, label)))),
        h("p", { class: "dim" }, "Your data is in ", h("span", { class: "mono", style: "overflow-wrap:anywhere" }, where), "."),
      ),
    ];
    if (r.unresolved_discrepancies && r.unresolved_discrepancies.length) nodes.push(notice("bad", h("strong", {}, "Differences found by the check:"), h("ul", {}, r.unresolved_discrepancies.map((d) => h("li", {}, d)))));
    if (verification) {
      const byObj = {};
      for (const c of verification.checks) (byObj[c.object || "all"] ||= []).push(c);
      nodes.push(
        h(
          "section",
          { class: "panel stack" },
          h("div", { class: "panel-title" }, h("h2", {}, icon("shield"), "Proof")),
          h("p", { class: "dim" }, "After the migration, the source and the destination were both read again and compared."),
          Object.entries(byObj).map(([obj, checks]) => {
            const passed = checks.filter((c) => c.passed).length;
            return h(
              "details",
              { open: passed !== checks.length },
              h("summary", {}, passed === checks.length ? badge("ok", `${passed}/${checks.length} checks passed`, "check") : badge("bad", `${checks.length - passed} of ${checks.length} checks failed`, "alert"), h("strong", {}, obj)),
              h("div", { class: "body" }, h("ul", { class: "list checks" }, checks.map((c) => h("li", { class: c.passed ? "pass" : "fail" }, icon(c.passed ? "check" : "x"), h("span", { class: "grow" }, h("strong", {}, CHECK_NAMES[c.name] || c.name), h("div", { class: "dim" }, c.detail)))))),
            );
          }),
        ),
      );
    }
    if (exc.length) {
      nodes.push(
        h(
          "section",
          { class: "panel stack" },
          h("div", { class: "panel-title" }, h("h2", {}, icon("file"), "Kept aside, not lost"), h("span", { class: "right dim" }, `first ${exc.length}`)),
          h("p", { class: "dim" }, "Records that could not be written and values with no place in the destination are stored on this computer with the reason."),
          h(
            "div",
            { class: "table-wrap" },
            h(
              "table",
              {},
              h("thead", {}, h("tr", {}, h("th", {}, "Data set"), h("th", {}, "Record"), h("th", {}, "What"), h("th", {}, "Why"))),
              h("tbody", {}, exc.map((x) => h("tr", {}, h("td", {}, x.source_object), h("td", { class: "mono" }, x.source_id), h("td", {}, x.kind === "archived_field" ? `field ${x.field}` : "whole record"), h("td", { class: "dim" }, x.reason)))),
            ),
          ),
        ),
      );
    }
    nodes.push(h("details", {}, h("summary", {}, h("strong", {}, "Full report (text)")), h("div", { class: "body stack" }, h("pre", { class: "report" }, rep.text), copyButton(rep.text))));
    nodes.push(h("div", { class: "row" }, h("a", { class: "btn primary", href: "#/new", onclick: () => (S.wiz = freshWizard()) }, icon("plus"), "Start another migration"), h("a", { class: "btn ghost", href: "#/" }, "Back to home")));
    proof.replaceChildren(...nodes);
  }

  const poll = async () => {
    try {
      job = await api("GET", `/jobs/${id}`);
      if (alive()) draw();
    } catch {
      /* transient: keep the last picture, try again on the next tick */
    }
  };
  const stopPolling = () => timer && (clearInterval(timer), (timer = null));

  // a job that was already over when the page opened is shown as it is, without a replay
  const opening = TERMINAL.includes(job.status);
  draw();
  if (opening) {
    Object.assign(shown, { read: real.read, written: real.written });
    if (replayRoute && job.status === "completed") replay(); // #/job/<id>/replay
    else draw();
  }
  raf = requestAnimationFrame(paint);
  if (!opening) timer = setInterval(poll, 250);

  return () => {
    stopPolling();
    raf && cancelAnimationFrame(raf);
    rain.destroy();
  };
}

const CHECK_NAMES = {
  written_matches_id_map: "Every write was recorded",
  identifier_coverage: "Every written record exists in the destination",
  no_duplicates: "No duplicates",
  referential_integrity: "Links between records point to the right place",
  sampled_field_comparison: "Sampled records match field by field",
  content_checksum: "Contents of all records match",
};

// ============================================================ boot
function initMotion() {
  let saved = null;
  try {
    saved = localStorage.getItem("ma.motion");
  } catch {
    /* storage unavailable */
  }
  S.motion = saved !== "off";
  S.bgRain = new CodeRain(document.getElementById("rain"), { fontSize: 16, fade: 0.05, fps: 24 });
  const btn = document.getElementById("motion-toggle");
  const apply = () => {
    S.bgRain.setEnabled(S.motion);
    btn.setAttribute("aria-pressed", String(S.motion));
    btn.textContent = `Animation: ${S.motion ? "on" : "off"}`;
  };
  btn.addEventListener("click", () => {
    S.motion = !S.motion;
    try {
      localStorage.setItem("ma.motion", S.motion ? "on" : "off");
    } catch {
      /* storage unavailable */
    }
    apply();
    route();
  });
  apply();
}

initMotion();
window.addEventListener("hashchange", route);
route();
