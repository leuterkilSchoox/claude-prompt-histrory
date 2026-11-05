const { invoke } = window.__TAURI__.core;
const $ = (s) => document.querySelector(s);

let sessions = [];
let hits = null;          // Map<path, snippet> while searching
let view = 'project';
let current = null;       // selected session
const closed = new Set(); // collapsed group keys

const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
};
const base = (p) => p.split(/[\\/]/).filter(Boolean).pop() || p || '(unknown)';
const fmt = (ts) => ts ? new Date(ts).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' }) : '';
const esc = (s) => s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

// Escape first, then add a little formatting. Never put raw transcript text into innerHTML.
function md(text) {
  const parts = esc(text).split(/```/);
  return parts.map((p, i) => {
    if (i % 2) return `<pre><code>${p.replace(/^[\w+-]*\n/, '')}</code></pre>`;
    return p
      .replace(/`([^`\n]+)`/g, '<code>$1</code>')
      .replace(/\*\*([^*\n]+)\*\*/g, '<b>$1</b>')
      .replace(/^#{1,6} (.*)$/gm, '<span class="h">$1</span>');
  }).join('');
}

function toast(msg) {
  const t = $('#toast');
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => (t.hidden = true), 4000);
}

// ---------- Sidebar ----------
function group(key, label, count, children, nested) {
  const d = el('details');
  d.open = !closed.has(key) || !!hits;
  d.ontoggle = () => (d.open ? closed.delete(key) : closed.add(key));
  const s = el('summary');
  s.append(el('span', null, label), el('span', 'count', String(count)));
  s.title = key;
  d.append(s, ...children);
  return d;
}

function item(s, sub) {
  const b = el('button', 'item' + (current && current.path === s.path ? ' active' : ''));
  b.append(el('span', 't', s.title), el('span', 's', hits ? hits.get(s.path) : sub));
  b.onclick = () => open(s);
  return b;
}

function bucket(list, keyFn) {
  const m = new Map();
  for (const s of list) for (const k of keyFn(s)) (m.get(k) || m.set(k, []).get(k)).push(s);
  return m; // insertion order follows `list`, which is sorted by last activity
}

function renderList() {
  const list = hits ? sessions.filter((s) => hits.has(s.path)) : sessions;
  const nav = $('#list');
  nav.replaceChildren();
  if (!list.length) return nav.append(el('p', 'empty', hits ? 'No matches.' : 'No conversations found.'));
  const branches = (s) => (s.branches.length ? s.branches : ['(no branch)']);

  if (view === 'project') {
    for (const [proj, ps] of bucket(list, (s) => [s.project])) {
      const inner = [...bucket(ps, branches)].map(([br, bs]) =>
        group(proj + '|' + br, '⑂ ' + br, bs.length, bs.map((s) => item(s, fmt(s.last_ts) + ' · ' + s.prompts + ' prompts')))
      );
      nav.append(group(proj, base(proj), ps.length, inner));
    }
  } else {
    for (const [br, bs] of bucket(list, branches)) {
      nav.append(group('b|' + br, '⑂ ' + br, bs.length, bs.map((s) => item(s, base(s.project) + ' · ' + fmt(s.last_ts)))));
    }
  }
}

async function loadList() {
  sessions = await invoke('list_sessions');
  if ($('#search').value.trim()) await runSearch();
  renderList();
}

async function runSearch() {
  const q = $('#search').value.trim();
  hits = q ? new Map((await invoke('search', { query: q })).map((h) => ['' + h.path, '…' + h.snippet])) : null;
  renderList();
}

// ---------- Chat ----------
function toolBlock(b) {
  const d = el('details', 'tool' + (b.is_error ? ' error' : ''));
  const firstLine = (b.text || '').split('\n').find((l) => /\w/.test(l)) || '';
  d.append(el('summary', null, `🔧 ${b.name}  ${firstLine.trim().slice(0, 90)}`));
  if (b.text) d.append(el('pre', null, b.text));
  if (b.result != null) d.append(el('pre', null, '→ ' + b.result));
  return d;
}

function renderChat(turns) {
  const chat = $('#chat');
  chat.replaceChildren();
  for (const t of turns) {
    const row = el('div', 'row ' + t.role);
    const bubble = el('div', 'bubble');
    let hasText = false;
    for (const b of t.blocks) {
      if (b.kind === 'text') {
        const m = el('div', 'md');
        m.innerHTML = md(b.text);
        bubble.append(m);
        hasText = true;
      } else if (b.kind === 'thinking') {
        bubble.append(el('div', 'md thinking', b.text));
      } else {
        bubble.append(toolBlock(b));
      }
    }
    if (!hasText) row.classList.add('tools-only');
    if (t.ts) bubble.append(el('div', 'time', fmt(t.ts)));
    row.append(el('div', 'avatar', t.role === 'user' ? 'You' : 'C'), bubble);
    chat.append(row);
  }
  chat.scrollTop = 0;
}

async function open(s) {
  current = s;
  renderList();
  $('#chat-head').hidden = false;
  $('#chat-title').textContent = s.title;
  const meta = $('#chat-meta');
  meta.replaceChildren(el('span', null, s.project), el('span', null, '· ' + fmt(s.first_ts) + ' →'));
  for (const b of s.branches) meta.append(el('span', 'chip', '⑂ ' + b));
  $('#chat').replaceChildren(el('p', 'empty', 'Loading…'));
  try {
    renderChat(await invoke('load_session', { path: s.path }));
  } catch (e) {
    $('#chat').replaceChildren(el('p', 'empty', 'Could not load: ' + e));
  }
}

// ---------- Wiring ----------
document.querySelectorAll('.tabs button').forEach((b) => {
  b.onclick = () => {
    document.querySelectorAll('.tabs button').forEach((x) => x.classList.toggle('active', x === b));
    view = b.dataset.view;
    renderList();
  };
});
let searchTimer;
$('#search').oninput = () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(runSearch, 200);
};
$('#refresh').onclick = loadList;
window.addEventListener('focus', loadList);
$('#show-tools').onchange = (e) => document.body.classList.toggle('hide-tools', !e.target.checked);
$('#export').onclick = async () => {
  try {
    toast('Saved to ' + (await invoke('export_md', { path: current.path, title: current.title })));
  } catch (e) {
    toast('Export failed: ' + e);
  }
};

loadList();
