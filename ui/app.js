const { invoke } = window.__TAURI__.core;
const $ = (id) => document.getElementById(id);
const apps = $("apps");
const message = $("message");

const STATUS = {
  chosen: () => "Program set by you",
  announced: (r) => `Installed, version ${r.version}` + (r.seen_at ? ` (last opened ${r.seen_at.slice(0, 10)})` : ""),
  installed: (r) => `Installed, version ${r.version || "unknown"}`,
  missing: () => "Not found",
  damaged: (r) => `Cannot read: ${r.note}`,
};
const OFFER = { install: "Install", update: "Update to" };

const say = (text) => { message.textContent = text; };
async function call(cmd, args) {
  try { return await invoke(cmd, args); } catch (e) { say(String(e)); throw e; }
}

function button(text, onClick, title) {
  const b = document.createElement("button");
  b.type = "button";
  b.textContent = text;
  if (title) b.title = title;
  b.addEventListener("click", onClick);
  return b;
}

// Install runs a program, so it takes a second click to confirm.
function offer(row) {
  const o = row.offer;
  if (!o) return [];
  const box = document.createElement("div");
  box.className = "offer";
  const line = document.createElement("div");
  line.textContent = `Catalog: ${o.version}` + (o.released ? ` (${o.released})` : "") + (o.notes ? ` - ${o.notes}` : "");
  box.append(line);
  if (o.problem) {
    const p = document.createElement("div");
    p.className = "problem";
    p.textContent = o.problem;
    box.append(p);
  } else if (OFFER[o.kind]) {
    const label = `${OFFER[o.kind]} ${o.version}`;
    const b = button(label, async () => {
      if (b.dataset.armed !== "1") { b.dataset.armed = "1"; b.textContent = `Click again to run the ${row.name} setup`; return; }
      b.disabled = true;
      try { say(await call("install_app", { name: row.name })); } catch { /* shown */ }
      b.disabled = false; b.dataset.armed = ""; b.textContent = label;
    });
    box.append(b);
  } else {
    box.append(o.kind === "same" ? "Up to date" : "Installed copy is newer");
  }
  return [box];
}

function card(row, names) {
  const el = document.createElement("section");
  el.className = "card";
  const h = document.createElement("h3");
  h.textContent = row.name;
  const status = document.createElement("div");
  status.className = `status ${row.status}`;
  status.textContent = STATUS[row.status](row);
  el.append(h, status);
  if (row.works_with.length) {
    const works = document.createElement("div");
    works.className = "works";
    works.append("Works with: ");
    row.works_with.forEach((name, i) => {
      if (i) works.append(", ");
      const b = document.createElement("b");
      b.textContent = name;
      if (!names.has(name.toLowerCase())) { b.className = "absent"; b.title = "Not on your list"; }
      works.append(b);
    });
    el.append(works);
  }
  const open = button("Open", async () => { say(""); try { await call("open_app", { name: row.name }); } catch { /* shown */ } });
  open.disabled = !row.can_open;
  const actions = document.createElement("div");
  actions.className = "row";
  actions.append(open, button("Edit", () => edit(row)), button("Remove", async () => {
    try { await call("remove_app", { name: row.name }); say(`${row.name} taken off the list (still installed).`); load(); } catch { /* shown */ }
  }, "Take it off the list; it stays installed"));
  el.append(actions, ...offer(row));
  return el;
}

function edit(row) {
  $("f-name").value = row?.name ?? "";
  $("f-program").value = row?.program ?? "";
  $("f-with").value = row?.works_with.join(", ") ?? "";
  $("form-title").textContent = row ? `Edit ${row.name}` : "Add an app";
  $("f-save").textContent = row ? "Save" : "Add";
  $("f-cancel").hidden = !row;
  if (row) $("f-name").focus();
}

async function load() {
  const hub = await invoke("hub");
  $("error").textContent = hub.error;
  $("config").textContent = hub.config;
  $("o-catalog").value = hub.catalog;
  $("o-blocked").value = hub.blocked.join("\n");
  $("catalog-notes").textContent = hub.catalog_notes.join(" ");
  let rows = [];
  try { rows = await invoke("list_apps"); } catch (e) { $("error").textContent = String(e); }
  const names = new Set(rows.map((r) => r.name.toLowerCase()));
  apps.replaceChildren(...rows.map((r) => card(r, names)));
  $("empty").hidden = rows.length > 0;
}

$("app-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const works_with = $("f-with").value.split(",").map((s) => s.trim()).filter(Boolean);
  try {
    await call("save_app", { name: $("f-name").value, program: $("f-program").value, worksWith: works_with });
    say(""); edit(null); load();
  } catch { /* shown */ }
});
$("f-cancel").addEventListener("click", () => edit(null));
$("catalog-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  try { await call("set_catalog", { folder: $("o-catalog").value }); say("Catalog folder saved."); load(); } catch { /* shown */ }
});
$("blocked-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  try { await call("set_blocked", { blocked: $("o-blocked").value.split("\n") }); say("Blocked list saved."); load(); } catch { /* shown */ }
});
$("refresh").addEventListener("click", () => { say(""); load(); });
load();
