import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { homeDir } from "@tauri-apps/api/path";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

type Category = "Pictures" | "Music" | "Movies" | "Documents" | "Archives" | "Installers";
interface CatalogEntry { category: Category; extensions: string[]; folder: string; }
interface Item { src: string; dest: string; category: Category; size: number; targetRoot: string; inPlace: boolean; subfolder?: string; }
interface Suggestion { src: string; subfolder: string; }
interface Root { path: string; kind: "folder" | "file"; }
interface AuthStatus { antInstalled: boolean; ssoConnected: boolean; hasApiKey: boolean; }
interface ScanResult { home: string; items: Item[]; skippedProjects: string[]; aiSortedSkipped: number; }
interface RunResult { count: number; errors: string[]; }

// Line icons in one ink: colour is kept for state, not for categories.
const META: Record<Category, { label: string; folder: string; icon: string }> = {
  Pictures: { label: "Images", folder: "Images", icon: '<rect x="2.5" y="4" width="15" height="12" rx="2"/><circle cx="7" cy="8.5" r="1.4"/><path d="M3 14.5l4.5-4 3 2.5 2.5-2 4 3.5"/>' },
  Music: { label: "Musique", folder: "Musique", icon: '<path d="M8 15V5l8-1.5v9.5"/><circle cx="6" cy="15" r="2"/><circle cx="14" cy="13" r="2"/>' },
  Movies: { label: "Vidéos", folder: "Films", icon: '<rect x="2.5" y="4.5" width="15" height="11" rx="2"/><path d="M8.5 7.5v5l4-2.5z"/>' },
  Documents: { label: "Documents", folder: "Documents", icon: '<path d="M5.5 2.5h6l3.5 3.5v11.5h-9.5z"/><path d="M11.5 2.5V6H15M8 10h5M8 13h5"/>' },
  Archives: { label: "Archives", folder: "Documents/Archives", icon: '<rect x="3.5" y="2.5" width="13" height="15" rx="1.5"/><path d="M10 2.5v2M10 6v2M10 9.5v2"/><rect x="8.5" y="12" width="3" height="3" rx=".5"/>' },
  Installers: { label: "Installeurs", folder: "Downloads/Installeurs", icon: '<path d="M10 3v9M6.5 8.5 10 12l3.5-3.5"/><path d="M3.5 12.5v3a1 1 0 0 0 1 1h11a1 1 0 0 0 1-1v-3"/>' },
};
// Real target folders come from the backend (macOS defaults or Linux XDG dirs, maybe localized)
const folderOf = (c: Category) => catalog.find((e) => e.category === c)?.folder ?? c;
const IS_MAC = /Mac/i.test(navigator.userAgent);
document.documentElement.dataset.platform = IS_MAC ? "mac" : "linux";
const ORDER: Category[] = ["Pictures", "Music", "Movies", "Documents", "Archives", "Installers"];
const STORAGE_KEY = "ranger.extensions";
const ROOTS_KEY = "ranger.roots";
const SORTED_KEY = "ranger.includeSorted";
const MAX_ROWS = 1500;

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const svg = (c: Category) =>
  `<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${META[c].icon}</svg>`;
const CROSS = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>';

let catalog: CatalogEntry[] = [];
let enabled = new Set<string>();
let openCustom: Category | null = null;
let home = "";
let roots: Root[] = [];
let lastAiSkipped = 0;
let items: Item[] = [];
let selected = new Set<string>();
let filter: Category | "all" = "all";
let query = "";

const rel = (p: string) => (home && p.startsWith(home) ? "~" + p.slice(home.length) : p);
const dir = (p: string) => rel(p).replace(/\/[^/]*$/, "") || "~";
const base = (p: string) => p.slice(p.lastIndexOf("/") + 1);
const finalDest = (i: Item) =>
  i.subfolder ? `${i.targetRoot}/${i.subfolder}/${base(i.src)}` : i.dest;
/** Will this item actually move when "Ranger" is clicked? */
const moving = (i: Item) => finalDest(i) !== i.src;
const sortedToggle = () => $<HTMLInputElement>("include-sorted");
const plural = (n: number, word: string) => `${n.toLocaleString("fr-FR")} ${word}${n > 1 ? "s" : ""}`;

function formatSize(n: number): string {
  const units = ["octets", "Ko", "Mo", "Go", "To"];
  let i = 0;
  while (n >= 1000 && i < units.length - 1) { n /= 1000; i++; }
  return `${n.toLocaleString("fr-FR", { maximumFractionDigits: i ? 1 : 0 })} ${units[i]}`;
}

function show(screen: "home" | "scanning" | "results" | "done") {
  for (const s of ["home", "scanning", "results", "done"]) $(`screen-${s}`).hidden = s !== screen;
  document.body.dataset.screen = screen;
  // Toolbar steps: 1 choose, 2 review (scanning is its loading state), 3 done
  const current = screen === "home" ? 1 : screen === "done" ? 3 : 2;
  document.querySelectorAll<HTMLElement>(".steps li").forEach((li) => {
    const n = Number(li.dataset.step);
    li.dataset.state = n < current ? "done" : n === current ? "current" : "todo";
    const step = li.querySelector(".step")!;
    n === current ? step.setAttribute("aria-current", "step") : step.removeAttribute("aria-current");
  });
  $<HTMLButtonElement>("step-choose").disabled = screen === "home" || screen === "scanning";
  const search = $<HTMLInputElement>("search");
  search.disabled = screen !== "results";
  if (screen !== "results" && search.value) { search.value = ""; query = ""; }
}

// ---------- Home: file type selection ----------

// Stores which extensions existed too, so ones added in later versions start enabled.
function saveTypes() {
  const known = catalog.flatMap((c) => c.extensions);
  try { localStorage.setItem(STORAGE_KEY, JSON.stringify({ enabled: [...enabled], known })); } catch { /* ignore */ }
}

function loadTypes() {
  const all = catalog.flatMap((c) => c.extensions);
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    const saved = raw ? JSON.parse(raw) : null;
    if (!saved) { enabled = new Set(all); return; }
    // Old format (plain array) predates archives/installers
    const known: string[] = Array.isArray(saved)
      ? all.filter((e) => !["Archives", "Installers"].includes(catalog.find((c) => c.extensions.includes(e))!.category))
      : saved.known;
    const on: string[] = Array.isArray(saved) ? saved : saved.enabled;
    enabled = new Set(all.filter((e) => on.includes(e) || !known.includes(e)));
  } catch {
    enabled = new Set(all);
  }
}

function extsOf(c: Category) {
  return catalog.find((e) => e.category === c)?.extensions ?? [];
}

function renderTypes() {
  $("types").replaceChildren(...ORDER.map((c) => {
    const exts = extsOf(c);
    const on = exts.filter((e) => enabled.has(e)).length;
    const row = document.createElement("div");
    row.className = `row ${on ? "" : "off"}`;
    row.innerHTML = `
      <span class="row-icon well cat-${c}">${svg(c)}</span>
      <span class="row-text">
        <span class="row-title">${META[c].label}</span>
        <span class="row-sub"></span>
      </span>
      <button type="button" class="link ext-toggle" aria-expanded="${openCustom === c}">Extensions</button>
      <button type="button" class="switch" role="switch" aria-checked="${on > 0}" aria-label="${META[c].label}"></button>`;
    const count = on === exts.length ? `${exts.length} extensions` : on === 0 ? "Désactivé" : `${on} sur ${exts.length} extensions`;
    row.querySelector(".row-sub")!.innerHTML = `${count}, vers <span class="mono">~/${folderOf(c)}</span>`;
    row.querySelector<HTMLButtonElement>(".switch")!.onclick = () => {
      const target = on === 0;
      for (const e of exts) target ? enabled.add(e) : enabled.delete(e);
      saveTypes();
      renderHome();
    };
    row.querySelector<HTMLButtonElement>(".ext-toggle")!.onclick = () => {
      openCustom = openCustom === c ? null : c;
      renderHome();
    };
    return row;
  }));

  const custom = $("custom");
  custom.hidden = openCustom === null;
  if (openCustom) {
    const c = openCustom;
    const exts = extsOf(c);
    custom.innerHTML = `
      <div class="custom-head">
        <h3>Extensions de ${META[c].label.toLowerCase()}</h3>
        <span><button type="button" class="link" data-act="all">Toutes</button><button type="button" class="link" data-act="none">Aucune</button></span>
      </div>
      <div class="chips"></div>`;
    custom.querySelector(".chips")!.replaceChildren(...exts.map((e) => {
      const chip = document.createElement("button");
      chip.type = "button";
      chip.className = "chip";
      chip.setAttribute("aria-pressed", String(enabled.has(e)));
      chip.textContent = `.${e}`;
      chip.onclick = () => {
        enabled.has(e) ? enabled.delete(e) : enabled.add(e);
        saveTypes();
        renderHome();
      };
      return chip;
    }));
    custom.querySelectorAll<HTMLButtonElement>("[data-act]").forEach((b) => {
      b.onclick = () => {
        for (const e of exts) b.dataset.act === "all" ? enabled.add(e) : enabled.delete(e);
        saveTypes();
        renderHome();
      };
    });
  }
}

function renderHome() {
  renderTypes();
  const count = enabled.size;
  $<HTMLButtonElement>("scan").disabled = count === 0;
  $("home-hint").textContent = count === 0
    ? "Active au moins un type de fichier."
    : `${plural(count, "extension")} sélectionnée${count > 1 ? "s" : ""}.`;
}

// ---------- Home: where to look ----------

function saveRoots() {
  try { localStorage.setItem(ROOTS_KEY, JSON.stringify(roots)); } catch { /* ignore */ }
}

function loadRoots() {
  try { roots = JSON.parse(localStorage.getItem(ROOTS_KEY) ?? "[]"); } catch { roots = []; }
}

const ICON = (d: string) => `<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" aria-hidden="true">${d}</svg>`;
const FOLDER_SVG = ICON('<path d="M2.5 5.5a1 1 0 0 1 1-1h4l1.5 1.5h7.5a1 1 0 0 1 1 1v8.5a1 1 0 0 1-1 1h-13a1 1 0 0 1-1-1z"/>');
const FILE_SVG = ICON('<path d="M5.5 2.5h6l3.5 3.5v11.5h-9.5z"/><path d="M11.5 2.5V6H15"/>');
const HOME_SVG = ICON('<path d="M3 9.5 10 3.5l7 6V17h-4.5v-4.5h-5V17H3z"/>');

function renderSources() {
  const list = $("source-list");
  $("reset-sources").hidden = roots.length === 0;
  if (!roots.length) {
    list.innerHTML = `<div class="row"><span class="row-icon">${HOME_SVG}</span>
      <span class="row-text"><span class="row-title">Tout mon dossier personnel</span>
      <span class="row-sub">Sauf les dossiers système, les outils de dev et les projets de code</span></span></div>`;
    return;
  }
  list.replaceChildren(...roots.map((r, idx) => {
    const row = document.createElement("div");
    row.className = "row";
    row.innerHTML = `<span class="row-icon">${r.kind === "folder" ? FOLDER_SVG : FILE_SVG}</span>
      <span class="row-text"><span class="row-title"></span><span class="row-sub mono"></span></span>
      <button type="button" class="icon-btn" aria-label="Retirer">${CROSS}</button>`;
    row.querySelector(".row-title")!.textContent = base(r.path);
    row.querySelector(".row-sub")!.textContent = dir(r.path);
    row.querySelector<HTMLButtonElement>(".icon-btn")!.setAttribute("aria-label", `Retirer ${base(r.path)}`);
    row.querySelector<HTMLButtonElement>(".icon-btn")!.onclick = () => {
      roots.splice(idx, 1);
      saveRoots();
      renderSources();
    };
    return row;
  }));
}

async function pick(kind: "folder" | "file") {
  const res = await openDialog({
    directory: kind === "folder",
    multiple: true,
    defaultPath: home,
    title: kind === "folder" ? "Dossiers à ranger" : "Fichiers à ranger",
  });
  if (!res) return;
  const picked = Array.isArray(res) ? res : [res];
  const outside = picked.filter((p) => !p.startsWith(home + "/"));
  for (const path of picked) {
    if (path.startsWith(home + "/") && !roots.some((r) => r.path === path)) roots.push({ path, kind });
  }
  saveRoots();
  renderSources();
  $("home-hint").textContent = outside.length
    ? "Seuls les éléments de ton dossier personnel peuvent être rangés."
    : "";
}

// ---------- Results ----------

function visible(): Item[] {
  const q = query.trim().toLocaleLowerCase("fr-FR");
  return items.filter((i) =>
    (filter === "all" || i.category === filter) &&
    (!q || rel(i.src).toLocaleLowerCase("fr-FR").includes(q) || rel(finalDest(i)).toLocaleLowerCase("fr-FR").includes(q)));
}

function renderCards() {
  const stats = new Map<Category | "all", { n: number; size: number }>([["all", { n: 0, size: 0 }]]);
  for (const i of items) {
    for (const key of ["all", i.category] as const) {
      const s = stats.get(key) ?? { n: 0, size: 0 };
      s.n++; s.size += i.size;
      stats.set(key, s);
    }
  }
  const keys: (Category | "all")[] = ["all", ...ORDER.filter((c) => stats.has(c))];
  $("cards").replaceChildren(...keys.map((c) => {
    const s = stats.get(c)!;
    const b = document.createElement("button");
    b.type = "button";
    b.className = "filter";
    b.setAttribute("aria-pressed", String(filter === c));
    b.innerHTML = `
      <span class="row-icon well ${c === "all" ? "cat-all" : `cat-${c}`}">${c === "all" ? HOME_SVG : svg(c)}</span>
      <span class="filter-name">${c === "all" ? "Tout" : META[c].label}</span>
      <span class="filter-count">${s.n.toLocaleString("fr-FR")}</span>
      <span class="filter-dest">${c === "all" ? formatSize(s.size) : `~/${folderOf(c)}`}</span>`;
    b.onclick = () => { filter = c; renderResults(); };
    return b;
  }));
}

function renderList() {
  const vis = visible();
  const frag = document.createDocumentFragment();
  for (const item of vis.slice(0, MAX_ROWS)) {
    const tr = document.createElement("tr");
    const on = selected.has(item.src);
    tr.className = `${on ? "is-on" : ""} ${item.subfolder && moving(item) ? "ai" : ""}`;
    tr.innerHTML = `
      <td class="c-check"><input type="checkbox" ${on ? "checked" : ""} /></td>
      <td><span class="cell-name"><span class="row-icon cat-${item.category}">${svg(item.category)}</span><span></span></span></td>
      <td><span class="path path--from"></span></td>
      <td><span class="path path--to"></span></td>
      <td class="c-size"><span class="size">${formatSize(item.size)}</span></td>`;
    tr.querySelector(".cell-name > span:last-child")!.textContent = base(item.src);
    const cb = tr.querySelector("input")!;
    cb.setAttribute("aria-label", base(item.src));
    tr.querySelector(".path--from")!.textContent = dir(item.src);
    const to = tr.querySelector<HTMLElement>(".path--to")!;
    if (moving(item)) {
      to.textContent = dir(finalDest(item));
      to.title = rel(finalDest(item));
      if (item.subfolder) to.insertAdjacentHTML("afterend", '<span class="ai-tag">Proposé par l’IA</span>');
    } else {
      to.className = "path path--stay";
      to.textContent = "Déjà rangé";
    }
    tr.querySelector<HTMLElement>(".path--from")!.title = rel(item.src);
    cb.onchange = () => {
      cb.checked ? selected.add(item.src) : selected.delete(item.src);
      tr.classList.toggle("is-on", cb.checked);
      renderSelection();
    };
    frag.append(tr);
  }
  $("list").replaceChildren(frag);
  const more = $("list-more");
  more.hidden = vis.length <= MAX_ROWS;
  more.textContent = `${plural(vis.length - MAX_ROWS, "autre fichier")}, non affichés mais inclus dans la sélection.`;
}

function renderSelection() {
  const chosen = items.filter((i) => selected.has(i.src));
  const toMove = chosen.filter(moving);
  const size = toMove.reduce((s, i) => s + i.size, 0);
  const stay = chosen.length - toMove.length;
  $("selection").textContent = `${plural(toMove.length, "fichier")} à déplacer, ${formatSize(size)}` +
    (stay ? `, ${stay} déjà rangé${stay > 1 ? "s" : ""}` : "");
  const apply = $<HTMLButtonElement>("apply");
  apply.textContent = toMove.length ? `Ranger ${plural(toMove.length, "fichier")}` : "Ranger";
  apply.disabled = toMove.length === 0;
  const vis = visible();
  const toggle = $<HTMLInputElement>("toggle-all");
  const n = vis.filter((i) => selected.has(i.src)).length;
  toggle.checked = vis.length > 0 && n === vis.length;
  toggle.indeterminate = n > 0 && n < vis.length;
}

function renderSkipped(paths: string[]) {
  const el = $<HTMLDetailsElement>("skipped");
  el.hidden = paths.length === 0;
  el.querySelector("summary")!.textContent = `${plural(paths.length, "dossier")} de projet ignoré${paths.length > 1 ? "s" : ""} pour ne rien casser`;
  el.querySelector("ul")!.replaceChildren(...paths.map((p) => {
    const li = document.createElement("li");
    li.textContent = rel(p);
    return li;
  }));
}

function renderResults() {
  const misplaced = items.filter((i) => !i.inPlace);
  const sorted = items.length - misplaced.length;
  const total = misplaced.reduce((s, i) => s + i.size, 0);
  const [figure, label] = misplaced.length
    ? [misplaced.length, `fichier${misplaced.length > 1 ? "s" : ""} à ranger`]
    : sorted ? [sorted, `fichier${sorted > 1 ? "s" : ""} déjà rangé${sorted > 1 ? "s" : ""}`] : [null, "Tout est déjà à sa place"];
  $("summary-title").innerHTML = figure === null
    ? label
    : `<span class="figure">${figure.toLocaleString("fr-FR")}</span> <span>${label}</span>`;
  $("summary-sub").textContent = [
    misplaced.length ? `${formatSize(total)} hors de leur dossier.` : "",
    sorted ? `${plural(sorted, "fichier")} déjà rangé${sorted > 1 ? "s" : ""} : clique sur « Organiser avec l’IA » pour ${sorted > 1 ? "les répartir" : "le placer"} dans de meilleurs sous-dossiers.` : "",
    !items.length ? "Aucun fichier des types choisis n’est mal rangé." : "",
    lastAiSkipped ? `${plural(lastAiSkipped, "fichier")} déjà classé${lastAiSkipped > 1 ? "s" : ""} par l’IA ignoré${lastAiSkipped > 1 ? "s" : ""}.` : "",
  ].filter(Boolean).join(" ");
  document.querySelector<HTMLElement>(".results-grid")!.hidden = items.length === 0;
  $("apply").hidden = items.length === 0;
  $("ai-run").hidden = items.length === 0;
  renderCards();
  renderList();
  renderSelection();
}

function setStatus(text: string, error = false) {
  const el = $("status");
  el.textContent = text;
  el.classList.toggle("error", error);
}

// ---------- AI ----------

const aiStatus = $("ai-status");

function renderAi() {
  const suggested = items.filter((i) => i.subfolder).length;
  $("ai-reset").hidden = suggested === 0;
  const folders = new Set(items.filter((i) => i.subfolder).map((i) => `${i.targetRoot}/${i.subfolder}`));
  if (suggested) aiStatus.textContent = `${plural(folders.size, "sous-dossier")} proposé${folders.size > 1 ? "s" : ""} pour ${plural(suggested, "fichier")}`;
}

let pendingAi = false;

function setSheetMsg(text: string, error = false) {
  const msg = $("key-msg");
  msg.textContent = text;
  msg.className = `small ${error ? "error" : ""}`;
}

async function refreshAuth(): Promise<AuthStatus> {
  const st = await invoke<AuthStatus>("auth_status");
  $("auth-connected").hidden = !st.ssoConnected;
  $("auth-sso").hidden = st.ssoConnected;
  $<HTMLButtonElement>("sso-login").disabled = !st.antInstalled;
  $("ant-missing").hidden = st.antInstalled;
  $("key-clear").hidden = !st.hasApiKey;
  $("key-state").textContent = st.hasApiKey ? "Une clé est enregistrée." : "";
  $("ai-settings").textContent = st.ssoConnected ? "Compte Anthropic connecté" : st.hasApiKey ? "Clé API enregistrée" : "Se connecter à Claude";
  return st;
}

function openSheet(message = "") {
  $<HTMLInputElement>("key-input").value = "";
  setSheetMsg(message, !!message);
  $("sheet").hidden = false;
  refreshAuth();
}

function closeSheet() { $("sheet").hidden = true; }

function connected() {
  closeSheet();
  if (pendingAi) { pendingAi = false; runAi(); }
}

async function ssoLogin() {
  const btn = $<HTMLButtonElement>("sso-login");
  btn.disabled = true;
  setSheetMsg("Termine la connexion dans ton navigateur…");
  try {
    await invoke("sso_login");
    await refreshAuth();
    setSheetMsg("");
    connected();
  } catch (e) {
    setSheetMsg(String(e), true);
  } finally {
    btn.disabled = false;
  }
}

async function ssoLogout() {
  await invoke("sso_logout").catch(() => {});
  await refreshAuth();
  setSheetMsg("Déconnecté.");
}

async function saveKey() {
  const key = $<HTMLInputElement>("key-input").value.trim();
  if (!key.startsWith("sk-ant-")) {
    setSheetMsg("La clé doit commencer par « sk-ant- ».", true);
    return;
  }
  try {
    await invoke("ai_set_key", { key });
    await refreshAuth();
    connected();
  } catch (e) {
    setSheetMsg(`Erreur : ${e}`, true);
  }
}

async function runAi() {
  const targets = items.filter((i) => selected.has(i.src));
  if (!targets.length) { aiStatus.textContent = "Sélectionne au moins un fichier."; return; }
  const auth = await invoke<AuthStatus>("auth_status");
  if (!auth.ssoConnected && !auth.hasApiKey) { pendingAi = true; openSheet(); return; }

  const btn = $<HTMLButtonElement>("ai-run");
  btn.disabled = true;
  btn.classList.add("working");
  $<HTMLButtonElement>("apply").disabled = true;
  aiStatus.classList.remove("error");
  aiStatus.textContent = `L’IA analyse ${plural(targets.length, "fichier")}…`;
  const unlisten = await listen<{ done: number; total: number }>("ai-progress", ({ payload }) => {
    if (payload.done < payload.total) aiStatus.textContent = `L’IA analyse… ${payload.done} / ${payload.total}`;
  });
  try {
    const suggestions = await invoke<Suggestion[]>("ai_suggest", { paths: targets.map((i) => i.src) });
    const bySrc = new Map(suggestions.map((s) => [s.src, s.subfolder.trim()]));
    for (const i of items) if (bySrc.has(i.src)) i.subfolder = bySrc.get(i.src) || undefined;
    renderList();
    renderAi();
  } catch (e) {
    aiStatus.textContent = String(e);
    aiStatus.classList.add("error");
    if (String(e).includes("Identifiants refusés")) openSheet("Tes identifiants ont été refusés. Reconnecte-toi ou change de clé.");
  } finally {
    unlisten();
    btn.disabled = false;
    btn.classList.remove("working");
    renderSelection();
  }
}

// ---------- Actions ----------

async function refreshAiMemory() {
  const n = await invoke<number>("ai_sorted_count").catch(() => 0);
  $("ai-memory").hidden = n === 0;
  $("ai-memory-text").textContent =
    `${plural(n, "fichier")} déjà classé${n > 1 ? "s" : ""} par l’IA ${n > 1 ? "seront ignorés" : "sera ignoré"}.`;
}

async function refreshUndo() {
  refreshAiMemory();
  const can = await invoke<boolean>("can_undo").catch(() => false);
  $("undo").hidden = !can;
  $("done-undo").hidden = !can;
}

async function scan() {
  show("scanning");
  try {
    const res = await invoke<ScanResult>("scan", {
      roots: roots.map((r) => r.path),
      extensions: [...enabled],
      includeSorted: sortedToggle().checked,
    });
    home = res.home;
    lastAiSkipped = res.aiSortedSkipped;
    items = res.items.sort((a, b) => a.src.localeCompare(b.src));
    selected = new Set(items.map((i) => i.src));
    filter = "all";
    setStatus("");
    aiStatus.textContent = "";
    $("ai-reset").hidden = true;
    renderSkipped(res.skippedProjects);
    renderResults();
    show("results");
  } catch (e) {
    show("home");
    $("home-hint").textContent = `Erreur : ${e}`;
  }
}

async function apply() {
  const chosen = items.filter((i) => selected.has(i.src) && moving(i));
  const paths = chosen.map((i) => i.src);
  if (!paths.length) return;
  const requests = chosen.map((i) => ({ src: i.src, subfolder: i.subfolder || null }));
  const btn = $<HTMLButtonElement>("apply");
  btn.disabled = true;
  setStatus("Rangement en cours…");
  try {
    const res = await invoke<RunResult>("apply", { requests });
    const moved = new Set(paths);
    items = items.filter((i) => !moved.has(i.src));
    selected = new Set(items.map((i) => i.src));
    $("done-title").textContent = `${plural(res.count, "fichier")} rangé${res.count > 1 ? "s" : ""}`;
    renderDoneList(res.errors.length ? [] : chosen);
    $("done-sub").textContent = res.errors.length
      ? `${plural(res.errors.length, "fichier")} n’a pas pu être déplacé : ${res.errors.slice(0, 2).join(", ")}`
      : chosen.some((i) => i.subfolder)
        ? "Les fichiers sont rangés dans les sous-dossiers proposés par l’IA."
        : "Chaque fichier est maintenant dans le bon dossier.";
    await refreshUndo();
    show("done");
  } catch (e) {
    setStatus(`Erreur : ${e}`, true);
    btn.disabled = false;
  }
}

/** Where the files went, one line per destination folder. */
function renderDoneList(moved: Item[]) {
  const list = $("done-list");
  const byDest = new Map<string, { n: number; size: number; category: Category }>();
  for (const i of moved) {
    const d = dir(finalDest(i));
    const e = byDest.get(d) ?? { n: 0, size: 0, category: i.category };
    e.n++; e.size += i.size;
    byDest.set(d, e);
  }
  list.hidden = byDest.size === 0;
  list.replaceChildren(...[...byDest].sort((a, b) => b[1].n - a[1].n).slice(0, 8).map(([d, e]) => {
    const row = document.createElement("div");
    row.className = "done-row";
    row.innerHTML = `<dt><span class="row-icon well cat-${e.category}">${svg(e.category)}</span><span class="mono"></span></dt>
      <dd><span class="done-n">${e.n.toLocaleString("fr-FR")}</span> <span class="muted">${formatSize(e.size)}</span></dd>`;
    row.querySelector("dt .mono")!.textContent = d;
    return row;
  }));
}

async function undo() {
  try {
    const res = await invoke<RunResult>("undo_last");
    $("done-title").textContent = `${plural(res.count, "fichier")} remis à sa place d’origine`;
    renderDoneList([]);
    $("done-sub").textContent = res.errors.length
      ? `${plural(res.errors.length, "fichier")} ignoré (déplacé ou supprimé entre-temps).`
      : "Le dernier rangement a été annulé.";
    await refreshUndo();
    show("done");
  } catch (e) {
    $("done-sub").textContent = `Erreur : ${e}`;
  }
}

$("scan").onclick = scan;
$("step-choose").onclick = () => { renderHome(); show("home"); };
$("settings").onclick = () => openSheet();
$<HTMLInputElement>("search").addEventListener("input", (e) => {
  query = (e.target as HTMLInputElement).value;
  renderList();
  renderSelection();
});
document.addEventListener("keydown", (e) => {
  const search = $<HTMLInputElement>("search");
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f" && !search.disabled) {
    e.preventDefault();
    search.focus();
    search.select();
  } else if (e.key === "Escape" && document.activeElement === search && search.value) {
    search.value = "";
    query = "";
    renderList();
    renderSelection();
  }
});
$("ai-run").onclick = runAi;
$("ai-settings").onclick = () => openSheet();
$("ai-reset").onclick = () => {
  for (const i of items) i.subfolder = undefined;
  aiStatus.textContent = "";
  renderList();
  renderAi();
};
$("key-save").onclick = saveKey;
$("key-cancel").onclick = () => { pendingAi = false; closeSheet(); };
$("key-clear").onclick = async () => {
  await invoke("ai_clear_key");
  await refreshAuth();
  setSheetMsg("Clé supprimée.");
};
$("sso-login").onclick = ssoLogin;
$("sso-logout").onclick = ssoLogout;
$("add-folders").onclick = () => pick("folder");
$("ai-forget").onclick = async (e) => {
  e.preventDefault(); // inside the checkbox <label>
  await invoke("ai_forget_sorted");
  refreshAiMemory();
};
sortedToggle().onchange = () => {
  try { localStorage.setItem(SORTED_KEY, sortedToggle().checked ? "1" : "0"); } catch { /* ignore */ }
};
$("add-files").onclick = () => pick("file");
$("reset-sources").onclick = () => { roots = []; saveRoots(); renderSources(); };
$("key-input").addEventListener("keydown", (e) => {
  if (e.key === "Enter") saveKey();
  if (e.key === "Escape") { pendingAi = false; closeSheet(); }
});
$("apply").onclick = apply;
$("undo").onclick = undo;
$("done-undo").onclick = undo;
$("back").onclick = () => show("home");
$("done-ok").onclick = () => { renderHome(); show("home"); };
$<HTMLInputElement>("toggle-all").onchange = (ev) => {
  const on = (ev.target as HTMLInputElement).checked;
  for (const i of visible()) on ? selected.add(i.src) : selected.delete(i.src);
  renderList();
  renderSelection();
};

(async () => {
  catalog = await invoke<CatalogEntry[]>("catalog");
  if (!IS_MAC) {
    $("ant-install").textContent = "go install github.com/anthropics/anthropic-cli/v2/cmd/ant@latest";
    $("keychain-note").textContent = "Elle est gardée dans le trousseau de ta session (GNOME Keyring ou KWallet).";
  }
  home = (await homeDir()).replace(/\/$/, "");
  loadTypes();
  loadRoots();
  renderSources();
  try { sortedToggle().checked = localStorage.getItem(SORTED_KEY) === "1"; } catch { /* ignore */ }
  refreshAuth().catch(() => {});
  renderHome();
  show("home");
  await refreshUndo();
})();
