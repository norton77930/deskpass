import { SETTINGS_TEXT, type SettingsText } from "./settings-strings.ts";

/**
 * Ticket 112: the settings page's static files, served by the settings server as they are: one HTML
 * page, its stylesheet and its script, no framework, no CDN, nothing fetched from anywhere but the
 * server itself (the server's Content-Security-Policy allows only that). They are kept here, in a
 * TypeScript module, so `npm run build:ts` puts them in dist/ with the rest and they are found the
 * same way from the repository (src/) and from an install (dist/), with no build step or package
 * file list of their own. The script is plain ES5-style JavaScript with no backticks and no `${`,
 * so it sits in a template literal unchanged.
 *
 * Every text comes from the settings table of the page's language (`SETTINGS_TEXT` or
 * `SETTINGS_TEXT_EN`): the HTML is filled with it here, and the script reads its `page` part from a
 * JSON block (`deskpass-text`) that is data, never run.
 */

function escapeHtml(text: string): string {
  return text.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");
}

/** JSON that can sit inside a `<script>` element: no `<` (so no `</script>`), and no line separators. */
function scriptJson(value: unknown): string {
  return JSON.stringify(value).replaceAll("<", "\\u003c").replaceAll("\u2028", "\\u2028").replaceAll("\u2029", "\\u2029");
}

/**
 * Ticket 117: which of a log row's tags (the words of the table's `logRows`) are good news,
 * a question you answered, or a refusal or failure; any other tag is shown in grey. The page gives
 * them their colours.
 */
function tagTones(LOG_TEXT: SettingsText["logRows"]): { ok: string[]; ask: string[]; bad: string[] } {
  const results = Object.entries(LOG_TEXT.results).filter(([name]) => name !== "ok").map(([, tag]) => tag);
  const answers = LOG_TEXT.approval.answers;
  return {
    ok: [LOG_TEXT.results.ok!],
    ask: [answers.allow_once!, answers.allow_always!, LOG_TEXT.approval.grantAllowAlways, LOG_TEXT.approval.sessionAllowOnce],
    bad: [...results, ...["refuse", "timeout", "unavailable", "refused_earlier", "blocked"].map((name) => answers[name]!), ...Object.values(LOG_TEXT.grantInvalid.reasons), LOG_TEXT.mismatch.result],
  };
}

/** The App page in the language of `T`; `pollMs` is how often the script asks the server for its state. */
export function appPageHtml(pollMs: number, T: SettingsText = SETTINGS_TEXT): string {
  const e = escapeHtml;
  const p = T.page;
  const l = T.log;
  const states = p.states
    .map((state) => `<div class="state"><span class="state-name state-${e(state.tier)}">${e(state.name)}</span><span class="muted small">${e(state.text)}</span></div>`)
    .join("");
  return `<!doctype html>
<html lang="${e(T.lang)}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${e(T.title)}</title>
<link rel="stylesheet" href="/app.css">
</head>
<body data-poll-ms="${pollMs}">
<header class="top">
  <div class="bar">
    <div class="brand">
      <svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="#2c5aa0" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="13" rx="2"></rect><path d="M8 21h8M12 17v4M9 10.5l2 2 4-4"></path></svg>
      <strong>${e(T.product)}</strong>
    </div>
    <nav class="tabs" aria-label="${e(p.nav.label)}"><a href="#apps" class="tab active" data-tab="apps" aria-current="page">${e(p.nav.apps)}</a><a href="#log" class="tab" data-tab="log">${e(p.nav.log)}</a></nav>
    <div class="effect" id="effect">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><circle cx="12" cy="12" r="9"></circle><path d="M8 12l3 3 5-6"></path></svg>
      ${e(p.inEffect)}
    </div>
  </div>
</header>
<div class="stopped" id="stopped" role="alert" hidden>${e(p.stopped)}</div>
<main>
  <div class="view" id="view-apps">
  <section class="card intro">
    <div class="intro-head">
      <h1>${e(p.heading)}</h1>
      <span class="muted small">${e(p.defaultLine)}</span>
    </div>
    <div class="states">${states}</div>
  </section>
  <section class="card setting">
    <label class="switch" for="allow-background"><input type="checkbox" id="allow-background" role="switch" aria-describedby="background-line" disabled><span>${e(p.background.label)}</span></label>
    <span class="muted small" id="background-line" role="status">${e(p.loading)}</span>
  </section>
  <div class="note" id="grants-note" role="status" hidden></div>
  <div class="columns">
    <section class="card list">
      <div class="list-head">
        <label for="search" class="visually-hidden">${e(p.search.label)}</label>
        <input id="search" type="search" autocomplete="off" placeholder="${e(p.search.placeholder)}">
        <div class="filters" id="filters" role="group" aria-label="${e(p.filtersLabel)}"></div>
        <span class="muted small list-note" id="list-note">${e(p.filterNotes.used)}</span>
      </div>
      <div class="scroll">
        <table>
          <thead><tr><th class="first">${e(p.columns.app)}</th><th>${e(p.columns.tier)}</th><th>${e(p.columns.alwaysAllow)}</th><th class="last">${e(p.columns.lastUsed)}</th></tr></thead>
          <tbody id="rows"></tbody>
        </table>
        <p class="empty" id="empty" hidden>${e(p.empty)}</p>
      </div>
      <div class="foot muted small" id="count">${e(p.loading)}</div>
    </section>
    <aside class="card detail" aria-label="${e(p.selected)}">
      <p class="muted" id="pick-hint">${e(p.pickHint)}</p>
      <div id="selected" hidden></div>
    </aside>
  </div>
  </div>
  <div class="view" id="view-log" hidden>
    <div class="log-head">
      <h1>${e(l.heading)}</h1>
      <div class="log-filters">
        <div class="days" id="log-days" role="group" aria-label="${e(l.daysLabel)}"></div>
        <label for="log-app">${e(l.appLabel)}</label>
        <select id="log-app"><option value="">${e(l.allApps)}</option></select>
      </div>
    </div>
    <div class="note" id="log-error" role="status" hidden>${e(l.loadFailed)}</div>
    <section class="card log" id="log-box">
      <div class="log-empty" id="log-empty" hidden></div>
      <div id="log-groups"><p class="empty">${e(l.loading)}</p></div>
    </section>
    <div class="note" id="log-truncated" role="status" hidden></div>
    <p class="muted small log-foot" id="log-foot"></p>
  </div>
</main>
<script type="application/json" id="deskpass-text">${scriptJson({ ...p, log: l, tones: tagTones(T.logRows) })}</script>
<script src="/app.js"></script>
</body>
</html>
`;
}

export const APP_CSS = `body{margin:0;background:#f4f5f7;color:#1c2230;font-family:"Noto Sans TC","Microsoft JhengHei",system-ui,sans-serif;font-size:15px;line-height:1.6}
a{color:#2c5aa0}a:hover{color:#1d3f73}
button{font:inherit;cursor:pointer}
[hidden]{display:none !important}
.muted{color:#5a6374}.small{font-size:13px}
.top{background:#fff;border-bottom:1px solid #d8dce4}
.bar{max-width:1200px;margin:0 auto;padding:14px 24px;display:flex;flex-wrap:wrap;align-items:center;gap:16px 28px}
.brand{display:flex;align-items:center;gap:10px}.brand strong{font-size:18px}
.tabs{display:flex;gap:4px}
.tab{text-decoration:none;padding:6px 14px;border-radius:6px;color:#5a6374}
.tab.active{background:#e5edf8;color:#1d3f73;font-weight:500}
.effect{margin-left:auto;display:flex;align-items:center;gap:8px;font-size:13px;color:#2b7649;background:#e3f1e8;padding:4px 12px;border-radius:99px}
.stopped{max-width:1152px;margin:18px auto 0;padding:12px 16px;border-radius:8px;background:#f7e2df;color:#8f2820;border:1px solid #e8b9b3}
main{max-width:1200px;margin:0 auto;padding:24px;display:grid;gap:18px}
.card{background:#fff;border:1px solid #d8dce4;border-radius:10px}
.intro{padding:18px 20px;display:grid;gap:12px}
.intro-head{display:flex;flex-wrap:wrap;justify-content:space-between;gap:8px;align-items:baseline}
h1{margin:0;font-size:20px}h2{margin:0;font-size:17px}
.states{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:10px}
@media (max-width:760px){.states{grid-template-columns:repeat(2,minmax(0,1fr))}}
.state{border:1px solid #d8dce4;border-radius:8px;padding:10px 12px;display:grid;gap:2px}
.state-name{font-weight:500}
.state-normal{color:#2c5aa0}.state-always{color:#2b7649}.state-high{color:#9c5d08}.state-blocked{color:#ab3027}
.setting{padding:14px 20px;display:flex;flex-wrap:wrap;gap:6px 16px;align-items:center}
.switch{display:flex;gap:10px;align-items:center;font-weight:500;cursor:pointer}
.switch input{width:18px;height:18px;margin:0;cursor:pointer;accent-color:#2c5aa0}
.switch input:disabled{cursor:not-allowed}
.switch input:focus-visible{outline:2px solid #2c5aa0;outline-offset:2px}
.note{padding:12px 16px;border-radius:8px;background:#faefda;color:#7a4705;border:1px solid #ecd3a3}
.columns{display:flex;flex-wrap:wrap;gap:18px;align-items:flex-start}
.list{flex:999 1 640px;min-width:0;display:grid}
.list-head{padding:14px 16px;display:flex;flex-wrap:wrap;gap:10px;align-items:center;border-bottom:1px solid #d8dce4}
.visually-hidden{position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0 0 0 0);white-space:nowrap}
#search{flex:1 1 220px;min-width:0;font:inherit;padding:8px 12px;border:1px solid #c5cbd6;border-radius:8px;background:#fff;color:#1c2230}
#search:focus-visible,.filterbtn:focus-visible{outline:2px solid #2c5aa0;outline-offset:2px}
.filters{display:flex;flex-wrap:wrap;gap:6px}
.filterbtn{padding:6px 12px;border-radius:99px;font-size:13px;min-height:36px;border:1px solid #c5cbd6;background:#fff;color:#3f4756}
.filterbtn[aria-pressed="true"]{border-color:#2c5aa0;background:#2c5aa0;color:#fff}
.list-note{flex-basis:100%}
.scroll{overflow-x:auto}
table{width:100%;border-collapse:collapse;font-size:14px}
th{text-align:left;color:#5a6374;font-size:13px;font-weight:500;padding:10px 12px}
td{padding:10px 12px;color:#3f4756}
th.first,td.first{padding-left:16px}th.last,td.last{padding-right:16px}
td.last{font-variant-numeric:tabular-nums}
tbody tr{border-top:1px solid #e6e9ef;cursor:pointer}
tbody tr:hover{background:#f7f9fc}
tbody tr.picked{background:#e5edf8}
.pick{all:unset;cursor:pointer;display:grid}
.pick:focus-visible{outline:2px solid #2c5aa0;outline-offset:2px;border-radius:4px}
.app-name{font-weight:500;color:#1c2230}
.app-pub{font-size:12px;color:#5a6374}
.pill{display:inline-block;padding:1px 10px;border-radius:99px;font-size:13px;font-weight:500}
.pill-normal{background:#e5edf8;color:#1d3f73}.pill-high{background:#faefda;color:#7a4705}.pill-blocked{background:#f7e2df;color:#8f2820}
.source{font-size:12px;color:#5a6374}
.empty{margin:0;padding:28px 16px;text-align:center;color:#5a6374}
.foot{padding:10px 16px;border-top:1px solid #d8dce4}
.detail{flex:1 1 320px;min-width:0;padding:18px;display:grid;gap:14px}
.detail p{margin:0}
#selected{display:grid;gap:14px}
.sel-head{display:grid;gap:2px}.sel-head h2{font-size:18px}
.sel-state{display:grid;gap:6px;font-size:14px}
.why{background:#f4f5f7;border-radius:8px;padding:10px 12px;font-size:14px;display:grid;gap:4px}
.why strong{font-size:13px;color:#5a6374;font-weight:500}
details{font-size:12px;color:#5a6374}summary{cursor:pointer}
dl.tech{margin:6px 0 0;display:grid;gap:4px}
dl.tech dt{font-weight:500}
dl.tech dd{margin:0;font-family:"IBM Plex Mono",Consolas,monospace;overflow-wrap:anywhere}
.actions{display:grid;gap:8px}
.action{display:grid;gap:1px;text-align:left;width:100%;min-height:44px;padding:9px 12px;border-radius:8px;border:1px solid #c5cbd6;background:#fff;color:#1c2230}
.action:hover:not(:disabled){background:#f7f9fc;border-color:#2c5aa0}
.action:focus-visible{outline:2px solid #2c5aa0;outline-offset:2px}
.action:disabled{cursor:not-allowed;background:#f4f5f7;border-color:#d8dce4;color:#5a6374}
.action-label{font-weight:500}.action-hint{font-size:12px;color:#5a6374}
.notice{margin:0;padding:10px 12px;border-radius:8px;background:#e3f1e8;color:#1f5c38;font-size:14px}
.notice-error{background:#f7e2df;color:#8f2820}
.notice-info{background:#e5edf8;color:#1d3f73}
.view{display:grid;gap:18px}
.log-head{display:flex;flex-wrap:wrap;gap:12px;align-items:center;justify-content:space-between}
.log-filters{display:flex;flex-wrap:wrap;gap:8px;align-items:center}
.log-filters label{font-size:13px;color:#5a6374}
.days{display:flex;flex-wrap:wrap;gap:6px}
.daybtn{padding:6px 12px;border-radius:99px;font-size:13px;min-height:36px;border:1px solid #c5cbd6;background:#fff;color:#3f4756}
.daybtn[aria-pressed="true"]{border-color:#2c5aa0;background:#2c5aa0;color:#fff}
.daybtn:focus-visible,.showday:focus-visible,select:focus-visible{outline:2px solid #2c5aa0;outline-offset:2px}
select{font:inherit;cursor:pointer;padding:6px 10px;border:1px solid #c5cbd6;border-radius:8px;background:#fff;color:#1c2230;min-height:36px}
.log-empty{padding:36px 20px;text-align:center;display:grid;gap:8px;justify-items:center}
.showday{margin-top:6px;padding:8px 16px;border-radius:8px;border:1px solid #2c5aa0;background:#fff;color:#2c5aa0;min-height:40px}
.log-group{padding:14px 20px;border-top:1px solid #e6e9ef;display:grid;gap:8px}
.log-group:first-child{border-top:0}
.log-group-head{display:flex;flex-wrap:wrap;gap:8px;align-items:baseline}
.log-rows{margin:0;padding:0;list-style:none;display:grid;gap:4px}
.log-row{display:grid;grid-template-columns:56px minmax(0,1fr) auto;gap:12px;font-size:14px;align-items:baseline}
.log-time{color:#5a6374;font-variant-numeric:tabular-nums}
.log-side{display:flex;gap:6px;align-items:baseline;justify-content:flex-end}
.mark{font-size:12px;padding:1px 8px;border-radius:99px;border:1px solid #c5cbd6;background:#fff;color:#3f4756}
.mark-background{border-color:#2c5aa0;color:#1d3f73}
.tag{font-size:12px;padding:1px 8px;border-radius:99px;background:#eceef2;color:#3f4756}
.tag-ok{background:#e3f1e8;color:#1f5c38}.tag-ask{background:#e5edf8;color:#1d3f73}.tag-bad{background:#f7e2df;color:#8f2820}
.log-foot{margin:0}
`;

/**
 * Ticket 116: the App list's filters and search, part of the page's script (and run on their own by
 * the tests): the server sends the apps the AI used (ticket 133: no others); a filter keeps the rows
 * of one kind ("Used by the AI" keeps them all), the search the rows whose name holds the text, without
 * regard to case.
 */
export const APP_FILTER_JS = `  var FILTERS = ["used", "mine", "blocked", "high"];
  function inFilter(app, id) {
    if (id === "mine") return app.source === "user" || app.alwaysAllow === "yes";
    if (id === "blocked" || id === "high") return app.tier === id;
    return true;
  }
  function matchesQuery(app, query) {
    var text = String(query || "").trim().toLowerCase();
    return text === "" || String(app.name).toLowerCase().indexOf(text) >= 0;
  }
  function filterCounts(apps) {
    var counts = {};
    FILTERS.forEach(function (id) {
      counts[id] = apps.filter(function (app) {
        return inFilter(app, id);
      }).length;
    });
    return counts;
  }
`;

export const APP_JS = `(function () {
  "use strict";
  var TEXT = JSON.parse(document.getElementById("deskpass-text").textContent);
  var POLL_MS = Number(document.body.getAttribute("data-poll-ms")) || 5000;
  var state = { tab: "apps", apps: [], picked: null, revision: null, stopped: false, timer: null, busy: false, notice: null, detailsOpen: false, focusAction: null, grants: null, filter: "used", query: "", confirm: null };
  // Ticket 115: how often the page asks whether the confirm window it opened has been answered.
  var CONFIRM_POLL_MS = 1000;
${APP_FILTER_JS}
  var RANK = { normal: 0, high: 1, blocked: 2 };

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined && text !== null) node.textContent = text;
    return node;
  }

  function fill(template, values) {
    return template.replace(/\\{(\\w+)\\}/g, function (whole, name) {
      return Object.prototype.hasOwnProperty.call(values, name) ? String(values[name]) : whole;
    });
  }

  function two(n) {
    return n < 10 ? "0" + n : String(n);
  }

  function formatTime(iso) {
    if (!iso) return TEXT.neverUsed;
    var date = new Date(iso);
    if (isNaN(date.getTime())) return TEXT.neverUsed;
    var day = (date.getMonth() + 1) + "/" + date.getDate();
    if (date.getFullYear() !== new Date().getFullYear()) day = date.getFullYear() + "/" + day;
    return day + " " + two(date.getHours()) + ":" + two(date.getMinutes());
  }

  function tierPill(app) {
    var pill = el("span", "pill pill-" + app.tier, TEXT.tiers[app.tier]);
    return pill;
  }

  function stop() {
    state.stopped = true;
    if (state.timer !== null) clearInterval(state.timer);
    document.getElementById("stopped").hidden = false;
    document.getElementById("effect").hidden = true;
  }

  function getJson(path) {
    return fetch(path, { cache: "no-store", credentials: "same-origin" }).then(function (response) {
      if (response.status === 403) {
        stop();
        throw new Error("forbidden");
      }
      if (!response.ok) throw new Error("status " + response.status);
      return response.json();
    });
  }

  // Ticket 116: the filter buttons, each with its count, and the line that says what the filter lists.
  function renderFilters() {
    var box = document.getElementById("filters");
    while (box.firstChild) box.removeChild(box.firstChild);
    var counts = filterCounts(state.apps);
    FILTERS.forEach(function (id) {
      var button = el("button", "filterbtn", fill(TEXT.filterCount, { name: TEXT.filters[id], n: counts[id] }));
      button.type = "button";
      button.setAttribute("data-filter", id);
      button.setAttribute("aria-pressed", id === state.filter ? "true" : "false");
      button.addEventListener("click", function () {
        state.filter = id;
        keepView(function () {
          renderFilters();
          renderRows();
        });
      });
      box.appendChild(button);
    });
    document.getElementById("list-note").textContent = TEXT.filterNotes[state.filter];
  }

  function renderRows() {
    var body = document.getElementById("rows");
    while (body.firstChild) body.removeChild(body.firstChild);
    var shown = state.apps.filter(function (app) {
      return inFilter(app, state.filter) && matchesQuery(app, state.query);
    });
    shown.forEach(function (app) {
      var row = el("tr", app.key === state.picked ? "picked" : "");
      var first = el("td", "first");
      var button = el("button", "pick");
      button.type = "button";
      button.setAttribute("aria-pressed", app.key === state.picked ? "true" : "false");
      button.setAttribute("data-key", app.key);
      button.appendChild(el("span", "app-name", app.name));
      button.appendChild(el("span", "app-pub", app.publisher));
      first.appendChild(button);
      row.appendChild(first);
      var tier = el("td");
      tier.appendChild(tierPill(app));
      tier.appendChild(document.createTextNode(" "));
      tier.appendChild(el("span", "source", TEXT.sources[app.source]));
      row.appendChild(tier);
      row.appendChild(el("td", "", TEXT.alwaysAllow[app.alwaysAllow]));
      row.appendChild(el("td", "last", formatTime(app.lastUsedAt)));
      row.addEventListener("click", function () {
        pick(app.key);
      });
      body.appendChild(row);
    });
    var empty = document.getElementById("empty");
    empty.hidden = shown.length > 0;
    empty.textContent = state.filter === "used" && state.query.trim() === "" ? TEXT.empty : TEXT.noMatch;
    document.getElementById("count").textContent = fill(shown.length === 1 ? TEXT.countOne : TEXT.count, { n: shown.length });
  }

  // Ticket 116: the search filters as it is typed; the poll's refresh leaves the box alone, so its text stays.
  document.getElementById("search").addEventListener("input", function (event) {
    state.query = event.target.value;
    renderRows();
  });

  function techRow(list, name, value) {
    list.appendChild(el("dt", "", name));
    list.appendChild(el("dd", "", value));
  }

  function renderSelected() {
    var holder = document.getElementById("selected");
    var app = null;
    state.apps.forEach(function (candidate) {
      if (candidate.key === state.picked) app = candidate;
    });
    while (holder.firstChild) holder.removeChild(holder.firstChild);
    document.getElementById("pick-hint").hidden = app !== null;
    holder.hidden = app === null;
    if (app === null) return;
    var head = el("div", "sel-head");
    head.appendChild(el("h2", "", app.name));
    head.appendChild(el("span", "muted small", app.publisher));
    holder.appendChild(head);
    var status = el("div", "sel-state");
    var tierLine = el("div");
    tierLine.appendChild(tierPill(app));
    tierLine.appendChild(document.createTextNode(" "));
    tierLine.appendChild(el("span", "source", TEXT.sources[app.source]));
    status.appendChild(tierLine);
    status.appendChild(el("div", "", fill(TEXT.alwaysAllowLine, { value: TEXT.alwaysAllow[app.alwaysAllow] })));
    holder.appendChild(status);
    var why = el("div", "why");
    why.appendChild(el("strong", "", TEXT.whyHeading));
    why.appendChild(el("span", "", app.why));
    holder.appendChild(why);
    holder.appendChild(renderActions(app));
    if (state.notice !== null && state.notice.key === app.key) {
      var tone = state.notice.tone || (state.notice.ok ? "ok" : "error");
      var notice = el("p", tone === "ok" ? "notice" : "notice notice-" + tone, state.notice.text);
      notice.id = "notice";
      notice.setAttribute("role", "status");
      notice.tabIndex = -1;
      holder.appendChild(notice);
    }
    var details = el("details");
    details.open = state.detailsOpen;
    details.addEventListener("toggle", function () {
      state.detailsOpen = details.open;
    });
    details.appendChild(el("summary", "", TEXT.details));
    var list = el("dl", "tech");
    techRow(list, TEXT.detailKey, app.key);
    techRow(list, TEXT.detailPath, app.path || TEXT.detailNoPath);
    techRow(list, TEXT.detailBuiltIn, app.builtIn ? TEXT.tiers[app.builtIn.tier] + " · " + app.builtIn.label : TEXT.detailBuiltInNone);
    techRow(list, TEXT.detailGrantCreated, app.grantCreatedAt ? formatTime(app.grantCreatedAt) : TEXT.detailGrantNone);
    details.appendChild(list);
    holder.appendChild(details);
  }

  // Ticket 113: the buttons that tighten, as the server rules them: a raise only above the app's tier
  // now, a revoke only of a grant that counts. A button that cannot be pressed says why.
  function actionList(app) {
    var A = TEXT.actions;
    var granted = app.alwaysAllow === "yes";
    var list = [];
    ["high", "blocked"].forEach(function (tier) {
      var allowed = RANK[tier] > RANK[app.tier];
      list.push({
        id: tier,
        label: A[tier],
        route: "/api/raise",
        body: { key: app.key, tier: tier },
        disabled: !allowed,
        hint: allowed ? (granted ? A.nowRevokes : A.now) : tier === "high" ? A.alreadyHigh : A.alreadyBlocked
      });
    });
    list.push({ id: "revoke", label: A.revoke, route: "/api/revoke", body: { key: app.key }, disabled: !granted, hint: granted ? A.revokeNow : A.noGrant });
    // Ticket 115: Restore default loosens: the server only opens the native confirm window for it. Ticket 125:
    // a raise of the user's that is not above the built-in tier (no longer written) is cleared the same way.
    var mine = app.raised === "high" || app.raised === "blocked";
    list.push({ id: "restore", label: A.restore, route: "/api/restore", body: { key: app.key }, disabled: !mine, hint: !mine ? A.noRaise : app.source === "user" ? A.restoreHint : A.restoreIdleHint });
    // The server writes only over a grants file that verified.
    if (state.grants !== "valid") {
      list.forEach(function (action) {
        action.disabled = true;
        action.hint = A.grantsInvalid;
      });
    }
    return list;
  }

  function renderActions(app) {
    var box = el("div", "actions");
    box.setAttribute("role", "group");
    box.setAttribute("aria-label", TEXT.actions.label);
    actionList(app).forEach(function (action) {
      var button = el("button", "action");
      button.type = "button";
      button.setAttribute("data-action", action.id);
      button.disabled = action.disabled || state.busy;
      button.appendChild(el("span", "action-label", action.label));
      // While a confirm window is open every button waits; the one that opened it says what to do.
      var hintText = action.hint;
      if (state.confirm !== null) {
        if (action.id === "restore" && state.confirm.key === app.key) hintText = TEXT.actions.confirming;
      } else if (state.busy && !action.disabled) {
        hintText = TEXT.actions.working;
      }
      var hint = el("span", "action-hint", hintText);
      hint.id = "hint-" + action.id;
      button.setAttribute("aria-describedby", hint.id);
      button.appendChild(hint);
      button.addEventListener("click", function () {
        act(app, action);
      });
      box.appendChild(button);
    });
    return box;
  }

  function act(app, action) {
    if (state.busy || action.disabled || state.stopped) return;
    state.busy = true;
    state.notice = null;
    state.focusAction = action.id;
    keepView(renderSelected);
    fetch(action.route, { method: "POST", cache: "no-store", credentials: "same-origin", headers: { "content-type": "application/json" }, body: JSON.stringify(action.body) })
      .then(function (response) {
        if (response.status === 403) {
          stop();
          return;
        }
        return response.json().then(null, function () {
          return null;
        }).then(function (data) {
          var ok = response.ok && data !== null && data.ok === true;
          // Ticket 115: the server opened the confirm window (202); the page waits for its answer.
          if (ok && response.status === 202 && data.pending === true && typeof data.id === "string") {
            state.confirm = { id: data.id, key: app.key };
            state.notice = { key: app.key, tone: "info", text: typeof data.message === "string" ? data.message : TEXT.actions.confirming };
            keepView(renderSelected);
            return waitConfirm(app.key);
          }
          state.notice = { key: app.key, ok: ok, text: data !== null && typeof data.message === "string" ? data.message : TEXT.actions.failed };
        });
      }, function () {
        state.notice = { key: app.key, ok: false, text: TEXT.actions.failed };
      })
      .then(function () {
        state.busy = false;
        if (state.stopped) return;
        return loadApps();
      })
      .then(null, function () {
        state.busy = false;
        keepView(renderSelected);
      })
      .then(function () {
        state.focusAction = null;
      });
  }

  // Ticket 115: asks the server about the open confirm until it is answered, then puts its result
  // in the notice (allowed, cancelled, timeout, changed or failed). Settles in every case.
  function waitConfirm(key) {
    return new Promise(function (resolve) {
      function finish(tone, text) {
        state.confirm = null;
        if (text !== null) state.notice = { key: key, tone: tone, text: text };
        resolve();
      }
      function ask() {
        if (state.stopped || state.confirm === null) return finish("error", null);
        getJson("/api/restore?id=" + encodeURIComponent(state.confirm.id)).then(function (data) {
          if (data.state === "pending") {
            setTimeout(ask, CONFIRM_POLL_MS);
            return;
          }
          // Ticket 124: another tab's newer confirm replaced this one: not a failure; the App list is reloaded after it.
          var tone = data.state === "allowed" ? "ok" : data.state === "cancelled" || data.state === "timeout" || data.state === "superseded" ? "info" : "error";
          finish(tone, typeof data.message === "string" ? data.message : TEXT.actions.failed);
        }, function (error) {
          if (state.stopped) return finish("error", null);
          // The confirm is no longer known (the server started again): nothing more will come of it.
          if (error && error.message === "status 404") return finish("error", TEXT.actions.failed);
          setTimeout(ask, CONFIRM_POLL_MS);
        });
      }
      setTimeout(ask, CONFIRM_POLL_MS / 2);
    });
  }

  // Ticket 126: a confirm window already open (this page was reloaded while it was, or another tab
  // opened it), as /api/apps says: the page waits for that one, as if it had opened it, and asks
  // for no second window; its app is picked when none is.
  function resumeConfirm(open) {
    if (state.busy || state.confirm !== null || state.stopped) return;
    state.busy = true;
    state.confirm = { id: open.id, key: open.key };
    if (state.picked === null) state.picked = open.key;
    state.notice = { key: open.key, tone: "info", text: typeof open.message === "string" ? open.message : TEXT.actions.confirming };
    keepView(function () {
      renderRows();
      renderSelected();
    });
    waitConfirm(open.key)
      .then(function () {
        state.busy = false;
        if (state.stopped) return;
        return loadApps();
      })
      .then(null, function () {
        state.busy = false;
        keepView(renderSelected);
      });
  }

  function pick(key) {
    if (key !== state.picked) state.notice = null;
    state.picked = key;
    renderRows();
    renderSelected();
  }

  function pickButton(key) {
    var buttons = document.querySelectorAll("#rows .pick");
    for (var i = 0; i < buttons.length; i += 1) if (buttons[i].getAttribute("data-key") === key) return buttons[i];
    return null;
  }

  // Ticket 113: a refresh (the poll seeing grants.json change elsewhere, or a change made here)
  // re-renders in place: the selection is kept by key, and the scroll position and the focus are
  // put back; a focused button that is now disabled hands the focus to the result line.
  function keepView(render) {
    var scroller = document.querySelector(".scroll");
    var x = window.scrollX;
    var y = window.scrollY;
    var left = scroller.scrollLeft;
    var top = scroller.scrollTop;
    var active = document.activeElement;
    var focusKey = active && active.getAttribute ? active.getAttribute("data-key") : null;
    var focusAction = active && active.getAttribute ? active.getAttribute("data-action") : null;
    var focusFilter = active && active.getAttribute ? active.getAttribute("data-filter") : null;
    // While a change runs its button is disabled and has lost the focus; it comes back once the change is done.
    if (focusKey === null && focusAction === null && focusFilter === null) focusAction = state.focusAction;
    render();
    scroller.scrollLeft = left;
    scroller.scrollTop = top;
    window.scrollTo(x, y);
    var target = null;
    if (focusKey !== null) target = pickButton(focusKey);
    else if (focusFilter !== null) target = document.querySelector('#filters [data-filter="' + focusFilter + '"]');
    else if (focusAction !== null) {
      target = document.querySelector('#selected [data-action="' + focusAction + '"]');
      if (target && target.disabled && !state.busy) target = document.getElementById("notice");
    }
    if (target && !target.disabled) target.focus({ preventScroll: true });
  }

  // Ticket 146: the Allow background operation switch. The server writes settings.json at once and the MCP server
  // reads it at its next tool call; a change that was not made puts the switch back.
  // Ticket 153: unreadable is settings.json that cannot be read; the switch is then neither on nor off here, and stays usable to rewrite the file.
  var settingState = { allowBackground: null, unreadable: false, busy: false, failed: false };

  function renderBackground() {
    var B = TEXT.background;
    var box = document.getElementById("allow-background");
    box.checked = settingState.allowBackground === true;
    box.disabled = (settingState.allowBackground === null && !settingState.unreadable) || settingState.busy || state.stopped;
    var line = document.getElementById("background-line");
    if (settingState.busy) line.textContent = B.working;
    else if (settingState.failed) line.textContent = B.failed;
    else if (settingState.unreadable) line.textContent = B.unreadable;
    else if (settingState.allowBackground !== null) line.textContent = settingState.allowBackground ? B.on : B.off;
  }

  function loadSettings() {
    return getJson("/api/settings").then(function (data) {
      if (settingState.busy) return;
      settingState.unreadable = data.unreadable === true;
      settingState.allowBackground = settingState.unreadable ? null : data.allowBackground === true;
      settingState.failed = false;
      renderBackground();
    });
  }

  document.getElementById("allow-background").addEventListener("change", function (event) {
    if (settingState.busy || state.stopped || (settingState.allowBackground === null && !settingState.unreadable)) return;
    var wanted = event.target.checked;
    settingState.busy = true;
    settingState.failed = false;
    renderBackground();
    fetch("/api/settings", { method: "POST", cache: "no-store", credentials: "same-origin", headers: { "content-type": "application/json" }, body: JSON.stringify({ allow_background: wanted }) })
      .then(function (response) {
        if (response.status === 403) {
          stop();
          return;
        }
        return response.json().then(null, function () {
          return null;
        }).then(function (data) {
          if (response.ok && data !== null && data.ok === true) {
            settingState.allowBackground = data.allowBackground === true;
            settingState.unreadable = false;
          } else settingState.failed = true;
        });
      }, function () {
        settingState.failed = true;
      })
      .then(function () {
        settingState.busy = false;
        renderBackground();
        var box = document.getElementById("allow-background");
        if (!box.disabled) box.focus({ preventScroll: true });
      });
  });

  function loadApps() {
    var settingsLoaded = loadSettings();
    return Promise.all([settingsLoaded, loadAppList()]);
  }

  function loadAppList() {
    return getJson("/api/apps").then(function (data) {
      keepView(function () {
        state.apps = data.apps;
        state.grants = data.grants.state;
        var note = document.getElementById("grants-note");
        note.textContent = data.grants.message || "";
        note.hidden = !data.grants.message;
        renderFilters();
        renderRows();
        renderSelected();
      });
      if (data.confirm && typeof data.confirm.id === "string") resumeConfirm(data.confirm);
    });
  }

  // Ticket 117: the Log tab. The tab is in the URL hash (#log; anything else is the App tab). The
  // chosen day (null: today, whatever today is), the App filter and the scroll position survive the
  // poll: a refresh whose answer is the same as the last one draws nothing.
  var L = TEXT.log;
  var APP_TITLE = document.title;
  var logState = { list: null, day: null, app: "", last: "", optionsSig: "", seq: 0 };
  var tones = {};
  ["ok", "ask", "bad"].forEach(function (name) {
    TEXT.tones[name].forEach(function (tag) {
      tones[tag] = "tag-" + name;
    });
  });

  function monthDay(day) {
    return Number(day.slice(5, 7)) + "/" + Number(day.slice(8, 10));
  }

  function localToday() {
    var now = new Date();
    return now.getFullYear() + "-" + two(now.getMonth() + 1) + "-" + two(now.getDate());
  }

  function clear(node) {
    while (node.firstChild) node.removeChild(node.firstChild);
  }

  function keepLogView(render) {
    var x = window.scrollX;
    var y = window.scrollY;
    var active = document.activeElement;
    var focusDay = active && active.getAttribute ? active.getAttribute("data-day") : null;
    var focusSelect = !!active && active.id === "log-app";
    render();
    window.scrollTo(x, y);
    var target = null;
    if (focusDay !== null) target = document.querySelector('#log-days [data-day="' + focusDay + '"]');
    else if (focusSelect) target = document.getElementById("log-app");
    if (target) target.focus({ preventScroll: true });
  }

  function renderLogEmpty(list, day, data) {
    var box = document.getElementById("log-empty");
    clear(box);
    var empty = data.groups.length === 0;
    box.hidden = !empty;
    if (!empty) return;
    if (day !== list.today || logState.app !== "") {
      box.appendChild(el("strong", "", L.emptyDay));
      return;
    }
    var other = list.latest !== null && list.latest !== list.today;
    box.appendChild(el("strong", "", other ? L.emptyToday : L.emptyWeek));
    box.appendChild(el("span", "muted small", L.emptyHint));
    if (other) {
      var button = el("button", "showday", fill(L.showDay, { date: monthDay(list.latest) }));
      button.type = "button";
      button.addEventListener("click", function () {
        chooseDay(list.latest);
      });
      box.appendChild(button);
    }
  }

  function renderLog(list, day, data) {
    var days = document.getElementById("log-days");
    clear(days);
    list.days.forEach(function (name) {
      var button = el("button", "daybtn", name === list.today ? fill(L.today, { date: monthDay(name) }) : monthDay(name));
      button.type = "button";
      button.setAttribute("data-day", name);
      button.setAttribute("aria-pressed", name === day ? "true" : "false");
      button.addEventListener("click", function () {
        chooseDay(name);
      });
      days.appendChild(button);
    });

    var select = document.getElementById("log-app");
    var signature = JSON.stringify(data.apps);
    if (signature !== logState.optionsSig) {
      logState.optionsSig = signature;
      clear(select);
      var all = el("option", "", L.allApps);
      all.value = "";
      select.appendChild(all);
      data.apps.forEach(function (app) {
        var option = el("option", "", app.name);
        option.value = app.key;
        select.appendChild(option);
      });
    }
    select.value = logState.app;

    renderLogEmpty(list, day, data);

    var groups = document.getElementById("log-groups");
    clear(groups);
    // The newest group first; the rows of a group in the order they happened.
    data.groups.slice().reverse().forEach(function (group) {
      var box = el("div", "log-group");
      var head = el("div", "log-group-head");
      head.appendChild(el("strong", "", group.app));
      var span = group.first !== group.last ? L.summarySpan : group.count === 1 ? L.summaryOne : L.summary;
      head.appendChild(el("span", "muted small", fill(span, { n: group.count, first: group.first, last: group.last })));
      box.appendChild(head);
      var rows = el("ol", "log-rows");
      group.rows.forEach(function (row) {
        var item = el("li", "log-row");
        item.appendChild(el("span", "log-time", row.time));
        item.appendChild(el("span", "", row.sentence));
        // Ticket 146: how the action was delivered (Background or Foreground), beside its result.
        if (row.mark || row.tag) {
          var side = el("span", "log-side");
          if (row.mark) side.appendChild(el("span", "mark mark-" + row.delivery, row.mark));
          if (row.tag) side.appendChild(el("span", "tag " + (tones[row.tag] || ""), row.tag));
          item.appendChild(side);
        }
        rows.appendChild(item);
      });
      box.appendChild(rows);
      groups.appendChild(box);
    });

    var cut = document.getElementById("log-truncated");
    cut.hidden = !data.truncated;
    cut.textContent = data.truncated ? fill(L.truncated, { n: data.rowsMax }) : "";
    document.getElementById("log-foot").textContent = list.retention + L.merged;
  }

  // The chosen day's groups; a day or an App that is no longer on offer (404) falls back to today, then to all Apps.
  function fetchDay() {
    var list = logState.list;
    var day = logState.day === null ? list.today : logState.day;
    var seq = (logState.seq += 1);
    return getJson("/api/log?day=" + encodeURIComponent(day) + (logState.app ? "&app=" + encodeURIComponent(logState.app) : "")).then(function (data) {
      if (seq !== logState.seq) return;
      document.getElementById("log-error").hidden = true;
      var signature = JSON.stringify([list, day, logState.app, data]);
      if (signature === logState.last) return;
      logState.last = signature;
      keepLogView(function () {
        renderLog(list, day, data);
      });
    }, function (error) {
      if (seq !== logState.seq) return;
      if (error && error.message === "status 404") {
        if (logState.app !== "") logState.app = "";
        else if (logState.day !== null) logState.day = null;
        else throw error;
        return fetchDay();
      }
      throw error;
    });
  }

  function loadLog() {
    return getJson("/api/log/days").then(function (list) {
      logState.list = list;
      if (logState.day !== null && (logState.day === list.today || list.days.indexOf(logState.day) < 0)) {
        logState.day = null;
        logState.app = "";
      }
      return fetchDay();
    });
  }

  function chooseDay(day) {
    logState.day = day === logState.list.today ? null : day;
    logState.app = "";
    fetchDay().then(null, handleError);
  }

  document.getElementById("log-app").addEventListener("change", function (event) {
    logState.app = event.target.value;
    fetchDay().then(null, handleError);
  });

  function currentTab() {
    return window.location.hash === "#log" ? "log" : "apps";
  }

  function showTab() {
    state.tab = currentTab();
    document.getElementById("view-apps").hidden = state.tab !== "apps";
    document.getElementById("view-log").hidden = state.tab !== "log";
    document.title = state.tab === "log" ? L.title : APP_TITLE;
    var links = document.querySelectorAll(".tabs .tab");
    for (var i = 0; i < links.length; i += 1) {
      var on = links[i].getAttribute("data-tab") === state.tab;
      links[i].className = on ? "tab active" : "tab";
      if (on) links[i].setAttribute("aria-current", "page");
      else links[i].removeAttribute("aria-current");
    }
  }

  function reload() {
    return state.tab === "log" ? loadLog() : loadApps();
  }

  function handleError(error) {
    if (state.stopped) return;
    if (error instanceof TypeError) {
      stop();
      return;
    }
    if (state.tab === "log") document.getElementById("log-error").hidden = false;
    else document.getElementById("count").textContent = TEXT.loadFailed;
  }

  window.addEventListener("hashchange", function () {
    showTab();
    if (!state.stopped) reload().then(null, handleError);
  });

  function poll() {
    if (state.stopped) return;
    getJson("/api/status").then(function (status) {
      // Past midnight "today" is another day, though no file changed.
      var rolled = state.tab === "log" && logState.list !== null && logState.list.today !== localToday();
      if (status.revision === state.revision && !rolled) return null;
      return reload().then(function () {
        state.revision = status.revision;
      });
    }).catch(handleError);
  }

  // Ticket 132: a tab in the background polls only about once a minute; shown again, it asks at once.
  document.addEventListener("visibilitychange", function () {
    if (document.visibilityState === "visible") poll();
  });

  showTab();
  poll();
  state.timer = setInterval(poll, POLL_MS);
})();
`;
