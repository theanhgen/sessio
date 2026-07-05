#!/usr/bin/env node
// `sessions` — pick a past Claude session and resume it.
// ←→ project · ↑↓ move · type to filter · ↵ resume · esc quit. Live-refreshes.

import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import readline from 'node:readline';
import { spawnSync, spawn, execFile } from 'node:child_process';

const ROOT = path.join(os.homedir(), '.claude', 'projects');
const CAP = 300; // scan the 300 most-recent sessions; plenty for getting back into recent work

// archive is a sessio-local hide list only: ids live in ~/.claude/.sessio/archived.json.
// the transcript files are never touched, so `claude --resume` still works and Claude's own
// cleanupPeriodDays still applies — this declutters the list, it does not preserve sessions.
const SESSIO_DIR = path.join(os.homedir(), '.claude', '.sessio');
const ARCHIVE_FILE = path.join(SESSIO_DIR, 'archived.json');
const loadArchived = () => { try { return new Set(JSON.parse(fs.readFileSync(ARCHIVE_FILE, 'utf8'))); } catch { return new Set(); } };
const saveArchived = (set) => { try { fs.mkdirSync(SESSIO_DIR, { recursive: true }); fs.writeFileSync(ARCHIVE_FILE, JSON.stringify([...set])); } catch {} };
let archived = loadArchived();

// locate ripgrep for full-content search (ctrl-f); null -> feature disabled gracefully
const RG = (() => {
  for (const p of ['rg', '/opt/homebrew/bin/rg', '/usr/local/bin/rg', '/usr/bin/rg']) {
    try { if (spawnSync(p, ['--version']).status === 0) return p; } catch {}
  }
  return null;
})();

const proj = (d) => d.replace(/^-Users-[^-]+-/, '').split('-').slice(-2).join('/');
const ago = (ms) => {
  const s = (Date.now() - ms) / 1000;
  if (s < 3600) return Math.max(1, Math.round(s / 60)) + 'm';
  if (s < 86400) return Math.round(s / 3600) + 'h';
  return Math.round(s / 86400) + 'd';
};
const sizeFmt = (b) => b < 1024 ? b + 'B' : b < 1048576 ? Math.round(b / 1024) + 'K' : (b / 1048576).toFixed(1) + 'M';
const MON = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const ts = (iso) => { if (!iso) return ' '.repeat(12); const d = new Date(iso), p = (n) => String(n).padStart(2, '0'); return `${d.getDate()} ${MON[d.getMonth()]} ${p(d.getHours())}:${p(d.getMinutes())}`.padEnd(12); };

// one full pass over a transcript: cwd, first/last typed prompt, prompt count,
// custom title (user-set) and ai title (auto). Gated string.includes keeps giant
// tool-result lines from being JSON-parsed.
// light head read for the LIST: first typed prompt, cwd, branch, and any early title.
// stops after ~400 lines once first+cwd are known, so giant transcripts aren't fully read.
function head(file) {
  return new Promise((res) => {
    const rl = readline.createInterface({ input: fs.createReadStream(file) });
    let n = 0, first = null, firstTs = null, cwd = null, branch = null, custom = null, ai = null;
    rl.on('line', (l) => {
      n++;
      if (l.includes('-title"')) { try { const o = JSON.parse(l); if (o.type === 'custom-title' && o.customTitle) custom = o.customTitle; else if (o.type === 'ai-title' && o.aiTitle) ai = ai || o.aiTitle; } catch {} }
      else if (l.includes('"promptSource":"typed"')) {
        try { const o = JSON.parse(l); if (o.type === 'user' && o.message) { const c = o.message.content; if (typeof c === 'string' && c.trim() && !c.startsWith('<')) { if (!first) { first = c.trim(); firstTs = o.timestamp || null; } if (!cwd && o.cwd) cwd = o.cwd; if (!branch && o.gitBranch) branch = o.gitBranch; } } } catch {}
      }
      else if (!cwd && l.includes('"cwd":"')) { try { const o = JSON.parse(l); if (o.cwd) cwd = o.cwd; } catch {} }
      if (n >= 400 && first && cwd) rl.close();
    });
    const done = () => res({ first, firstTs, cwd, branch, custom, ai });
    rl.on('close', done); rl.on('error', done);
  });
}

// full read for the DETAIL panel of the highlighted session only (lazy): last prompt,
// prompt count, compact summary, last assistant reply, and any late-set title.
function detail(file) {
  return new Promise((res) => {
    const rl = readline.createInterface({ input: fs.createReadStream(file) });
    let cwd = null, first = null, last = null, firstTs = null, lastTs = null, count = 0, custom = null, ai = null, branch = null, summary = null, summaryTs = null, reply = null, replyTs = null;
    rl.on('line', (l) => {
      if (l.includes('-title"')) {
        try { const o = JSON.parse(l); if (o.type === 'custom-title' && o.customTitle) custom = o.customTitle; else if (o.type === 'ai-title' && o.aiTitle) ai = o.aiTitle; } catch {}
        return;
      }
      if (l.includes('"isCompactSummary":true')) { // auto-compact recap; keep the latest, strip boilerplate
        try {
          const o = JSON.parse(l);
          if (o.isCompactSummary === true) { // exact field, not a mention of the word
            const c = o.message && o.message.content; // string in some sessions, array of blocks in others
            const t = typeof c === 'string' ? c : Array.isArray(c) ? c.map((x) => (x && x.text) || '').join('\n\n') : '';
            if (t) { const i = t.indexOf('Summary:'); summary = (i >= 0 ? t.slice(i + 8) : t).trim(); summaryTs = o.timestamp || null; } // raw markdown, rendered at display
          }
        } catch {}
        return;
      }
      if (l.includes('"type":"assistant"') && l.includes('"type":"text"')) { // keep the latest assistant text reply
        try {
          const o = JSON.parse(l);
          if (o.type === 'assistant' && o.message && Array.isArray(o.message.content)) {
            const txt = o.message.content.filter((b) => b.type === 'text').map((b) => b.text).join('\n\n');
            if (txt.trim()) { reply = txt.trim(); replyTs = o.timestamp || null; } // raw markdown, rendered at display
            if (!cwd && o.cwd) cwd = o.cwd;
          }
        } catch {}
        return;
      }
      if (l.includes('"promptSource":"typed"')) {
        try {
          const o = JSON.parse(l);
          if (o.type === 'user' && o.message) {
            const c = o.message.content;
            if (typeof c === 'string' && c.trim() && !c.startsWith('<')) {
              count++;
              if (!first) { first = c.trim(); firstTs = o.timestamp || null; }
              last = c.trim(); lastTs = o.timestamp || null;
              if (!cwd && o.cwd) cwd = o.cwd;
              if (!branch && o.gitBranch) branch = o.gitBranch;
            }
          }
        } catch {}
        return;
      }
      if (!cwd && l.includes('"cwd":"')) { try { const o = JSON.parse(l); if (o.cwd) cwd = o.cwd; } catch {} }
    });
    const done = () => res({ cwd, first, last, firstTs, lastTs, count, custom, ai, branch, summary, summaryTs, reply, replyTs, _loaded: true });
    rl.on('close', done); rl.on('error', done);
  });
}

// cheap tail read (last ~64KB) to tell whether a session is "open": who spoke last,
// and — if Claude — whether it ended on a question / proposal you didn't answer.
const CTA = /\?\s*["')\]]*\s*$|\b(want me to|should i|shall i|do you want|let me know|ready to|next step|proceed\b|which (one|of)|confirm)\b/i;
function tail(file) {
  return new Promise((res) => {
    fs.stat(file, (e, st) => {
      if (e) return res({});
      const rl = readline.createInterface({ input: fs.createReadStream(file, { start: Math.max(0, st.size - 65536) }) });
      let reply = null, replyTs = null, userTs = null; // last assistant text vs last typed user prompt
      rl.on('line', (l) => {
        if (l.includes('"type":"assistant"') && l.includes('"type":"text"')) {
          try { const o = JSON.parse(l); if (o.type === 'assistant' && Array.isArray(o.message?.content)) { const t = o.message.content.filter((b) => b.type === 'text').map((b) => b.text).join('\n\n'); if (t.trim()) { reply = t.trim(); replyTs = o.timestamp || replyTs; } } } catch {}
        } else if (l.includes('"promptSource":"typed"')) {
          try { const o = JSON.parse(l); if (o.type === 'user' && typeof o.message?.content === 'string' && o.message.content.trim() && !o.message.content.startsWith('<')) userTs = o.timestamp || userTs; } catch {}
        }
      });
      const done = () => {
        const T = (x) => x ? new Date(x).getTime() : 0;
        let open = false, why = '';
        if (userTs && T(userTs) > T(replyTs)) { open = true; why = 'your prompt got no reply'; }
        else if (reply && CTA.test(reply.slice(-400))) { open = true; why = 'Claude asked / proposed next'; }
        res({ open, why });
      };
      rl.on('close', done); rl.on('error', done);
    });
  });
}

const gitCache = new Map();    // cwd -> {dirty, at}; re-check at most every 20s
const gitInflight = new Set(); // cwds with a background `git status` in progress (dedup)
// non-blocking: kick off `git status` in the background, update the cache when it returns.
// the running load() picks up the fresh value on a later tick — no synchronous stall.
function gitRefresh(cwd) {
  if (gitInflight.has(cwd)) return;
  gitInflight.add(cwd);
  execFile('git', ['-C', cwd, 'status', '--porcelain'], { encoding: 'utf8', timeout: 2000, maxBuffer: 1 << 20 }, (err, stdout) => {
    gitInflight.delete(cwd);
    gitCache.set(cwd, { dirty: !err && stdout.trim().length > 0, at: Date.now() });
  });
}
// return the best-known dirtiness immediately; refresh in the background when stale/unknown.
function gitDirty(cwd) {
  const c = gitCache.get(cwd);
  if (!c || Date.now() - c.at >= 20000) gitRefresh(cwd);
  return c ? c.dirty : false; // unknown until the first background check resolves
}

function wrap(text, width, maxLines) {
  const words = (text || '').replace(/\s+/g, ' ').trim().split(' ');
  const lines = []; let cur = '';
  for (const w of words) {
    if ((cur + ' ' + w).trim().length > width) { lines.push(cur); cur = w; }
    else cur = (cur + ' ' + w).trim();
    if (lines.length >= maxLines) break;
  }
  if (cur && lines.length < maxLines) lines.push(cur);
  const full = words.join(' ');
  if (lines.length === maxLines && full.length > lines.join(' ').length) lines[maxLines - 1] = lines[maxLines - 1].slice(0, width - 1) + '…';
  return lines.length ? lines : [''];
}

const hCache = new Map(); // id -> {mtime, ...head()}   list metadata (fast)
const dCache = new Map(); // id -> {mtime, ...detail()} preview metadata (lazy, per highlight)
async function load() {
  const rows = [];
  for (const d of fs.readdirSync(ROOT)) {
    const dir = path.join(ROOT, d);
    let st; try { st = fs.statSync(dir); } catch { continue; }
    if (!st.isDirectory()) continue;
    for (const f of fs.readdirSync(dir)) {
      if (!f.endsWith('.jsonl')) continue;
      let s; try { s = fs.statSync(path.join(dir, f)); } catch { continue; }
      if (s.isFile()) rows.push({ id: f.slice(0, -6), file: path.join(dir, f), mtime: s.mtimeMs, size: s.size, dir: d });
    }
  }
  rows.sort((a, b) => b.mtime - a.mtime);
  const items = [];
  for (const r of rows.slice(0, CAP)) {
    const c = hCache.get(r.id);
    const info = (c && c.mtime === r.mtime) ? c : { mtime: r.mtime, ...(await head(r.file)), ...(await tail(r.file)) };
    hCache.set(r.id, info);
    Object.assign(r, info);
    if (!r.first) continue; // skip empty (0-prompt) sessions
    r.title = r.custom || r.ai || null;
    r.name = r.title || r.first;
    // "Claude proposed next" is weak (most replies offer next steps) — only count it when
    // recent (last 3 days); unanswered prompts and git-WIP count at any age.
    if (r.open && r.why === 'Claude proposed next' && Date.now() - r.mtime > 3 * 86400000) r.open = false;
    if (r.open) r.openWhy = r.why;
    const d = dCache.get(r.id); // fold in detail if it's already been read for this session
    if (d && d.mtime === r.mtime) Object.assign(r, d);
    items.push(r);
    if (items.length >= CAP) break;
  }
  // git WIP: flag the most-recent session in each project whose folder has uncommitted changes
  const seenCwd = new Set();
  for (const r of items) {
    if (!r.cwd || seenCwd.has(r.cwd)) continue;
    seenCwd.add(r.cwd);
    if (gitDirty(r.cwd)) { r.open = true; r.openWhy = r.openWhy || 'uncommitted changes'; }
  }
  // one canonical label per project dir: prefer a sibling's real cwd basename (hyphens
  // intact); fall back to the dash-decoded name only if no session in the dir has a cwd.
  const dirLabel = new Map();
  for (const r of items) if (r.cwd && !dirLabel.has(r.dir)) dirLabel.set(r.dir, path.basename(r.cwd));
  for (const r of items) {
    r.project = dirLabel.get(r.dir) || proj(r.dir);
    r.hay = (r.project + ' ' + r.name + ' ' + r.first).toLowerCase();
  }
  return items;
}

process.stdout.write('loading…\r');
let items = await load();
if (!items.length) { console.log('No sessions found.'); process.exit(0); }

// --- picker ---
const out = process.stdout;
const D = '\x1b[2m', CY = '\x1b[36m', YE = '\x1b[33m', G = '\x1b[32m', O = '\x1b[38;5;208m', V = '\x1b[38;5;141m', B = '\x1b[1m', INV = '\x1b[7m', R = '\x1b[0m', CLR = '\x1b[2J\x1b[H';
const ACTIVE_MS = 5 * 60 * 1000;        // green dot: active (written in last 5 min)
const RECENT_MS = 24 * 60 * 60 * 1000;  // orange dot: recent (last 24h, but not active)
const HIDE = '\x1b[?25l', SHOW = '\x1b[?25h'; // hide/show the terminal cursor
process.on('exit', () => out.write(SHOW)); // always restore cursor, whatever the exit path

// minimal markdown -> ANSI renderer so replies/summaries look like Claude renders them
const stripAnsi = (s) => s.replace(/\x1b\[[0-9;]*m/g, '');
const inlineMd = (s) => s
  .replace(/\*\*(.+?)\*\*/g, `${B}$1${R}`)          // bold
  .replace(/__(.+?)__/g, `${B}$1${R}`)
  .replace(/`([^`]+)`/g, `${CY}$1${R}`)             // inline code
  .replace(/\[(.+?)\]\((?:.+?)\)/g, `${CY}$1${R}`); // links -> just the text
function wrapMd(text, width, first, cont) {
  const words = text.split(/\s+/).filter(Boolean);
  const lines = []; let line = first, started = false;
  for (const wd of words) {
    const cand = started ? line + ' ' + wd : line + wd;
    if (started && stripAnsi(cand).length > width) { lines.push(line); line = cont + wd; }
    else { line = cand; started = true; }
  }
  lines.push(line);
  return lines;
}
function cellFit(raw, wd) { // render a table cell to an exact visible width
  const styled = inlineMd(raw), vis = stripAnsi(styled);
  if (vis.length <= wd) return styled + ' '.repeat(wd - vis.length);
  return raw.slice(0, Math.max(0, wd - 1)) + '…'; // drop styling when truncating (rare)
}
function renderTable(rows, width) { // rows[0] = header, rest = data (separator already dropped)
  const ncol = Math.max(...rows.map((r) => r.length));
  rows = rows.map((r) => { const c = r.slice(); while (c.length < ncol) c.push(''); return c; });
  const widths = [];
  for (let i = 0; i < ncol; i++) widths[i] = Math.max(1, ...rows.map((r) => stripAnsi(inlineMd(r[i])).length));
  let over = widths.reduce((a, b) => a + b, 0) + 2 * (ncol - 1) - width; // shrink widest cols until it fits
  while (over > 0) { let mi = 0; for (let i = 1; i < ncol; i++) if (widths[i] > widths[mi]) mi = i; if (widths[mi] <= 6) break; widths[mi]--; over--; }
  const out = [rows[0].map((c, i) => `${B}${cellFit(c, widths[i])}${R}`).join('  ')];
  out.push(`${D}${'─'.repeat(Math.min(width, widths.reduce((a, b) => a + b, 0) + 2 * (ncol - 1)))}${R}`);
  for (let r = 1; r < rows.length; r++) out.push(rows[r].map((c, i) => cellFit(c, widths[i])).join('  '));
  return out;
}
function mdLines(text, width) {
  const res = [];
  const raw = String(text).split('\n');
  const isRow = (s) => /^\s*\|.*\|\s*$/.test(s);
  const isSep = (s) => s.includes('|') && /-/.test(s) && /^\s*\|?[\s:|-]+\|?\s*$/.test(s);
  const cells = (s) => s.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map((c) => c.trim());
  for (let i = 0; i < raw.length; i++) {
    let line = raw[i].replace(/\s+$/, '');
    if (isRow(line) && i + 1 < raw.length && isSep(raw[i + 1])) { // markdown table
      const block = [cells(line)];
      let j = i + 2;
      while (j < raw.length && isRow(raw[j])) { block.push(cells(raw[j])); j++; }
      renderTable(block, width).forEach((l) => res.push(l));
      i = j - 1; continue;
    }
    if (!line.trim()) { res.push(''); continue; }
    let m;
    if ((m = line.match(/^\s*#{1,6}\s+(.*)/))) { res.push(`${B}${inlineMd(m[1])}${R}`); continue; }
    if ((m = line.match(/^(\s*)[-*+]\s+(.*)/))) { const ind = m[1] || ''; wrapMd(inlineMd(m[2]), width, `${ind}• `, `${ind}  `).forEach((l) => res.push(l)); continue; }
    if ((m = line.match(/^(\s*)(\d+)\.\s+(.*)/))) { const ind = m[1] || '', pre = `${ind}${m[2]}. `; wrapMd(inlineMd(m[3]), width, pre, ' '.repeat(stripAnsi(pre).length)).forEach((l) => res.push(l)); continue; }
    wrapMd(inlineMd(line), width, '', '').forEach((l) => res.push(l));
  }
  const out = []; // collapse consecutive blanks, trim ends, so budget isn't spent on empty lines
  for (const l of res) { if (l === '' && (!out.length || out[out.length - 1] === '')) continue; out.push(l); }
  while (out.length && out[out.length - 1] === '') out.pop();
  return out;
}
let q = '', cur = 0, off = 0, pIdx = 0, limit = 12, expand = false; // limit = rows before "show more"; expand = full reply
let deep = null; // {query, ids:Set} when a content search is active
let searchGen = 0; // bumped on every query change / search; stale async rg results are dropped
const OPEN_TAB = '⏸ open';
const ARCHIVED_TAB = '🗄 archived';
// tabs are built from live (non-archived) sessions; the archived tab appears only while
// something is archived, and always sits last.
const tabsFor = (its) => {
  const live = its.filter((i) => !archived.has(i.id));
  return ['All', ...(live.some((i) => i.open) ? [OPEN_TAB] : []), ...new Set(live.map((i) => i.project)),
    ...(its.some((i) => archived.has(i.id)) ? [ARCHIVED_TAB] : [])];
};
let projects = tabsFor(items);
const view = () => {
  const p = projects[pIdx];
  let l;
  if (p === ARCHIVED_TAB) l = items.filter((i) => archived.has(i.id));      // archived tab: only archived
  else {
    l = p === 'All' ? items : p === OPEN_TAB ? items.filter((i) => i.open) : items.filter((i) => i.project === p);
    l = l.filter((i) => !archived.has(i.id));                              // every other tab: hide archived
  }
  if (deep) l = l.filter((i) => deep.ids.has(i.id));      // content match wins
  else if (q) l = l.filter((i) => i.hay.includes(q.toLowerCase())); // fast name/first filter
  return l;
};

// lazily full-read the highlighted session for its preview (count/last/reply/summary/title)
function ensureDetail() {
  const it = view()[cur];
  if (!it || it._loaded) return;
  const c = dCache.get(it.id);
  if (c && c.mtime === it.mtime) { Object.assign(it, c); it.name = (it.custom || it.ai) || it.name; return; }
  detail(it.file).then((d) => {
    const rec = { mtime: it.mtime, ...d };
    dCache.set(it.id, rec);
    Object.assign(it, d);
    if (it.custom || it.ai) { it.title = it.custom || it.ai; it.name = it.title; }
    draw();
  });
}

// content search: grep every transcript body for the term, keep matching session ids.
// only the CAP most-recent sessions stay in the list, but rg searches all of them on disk.
// async so the live UI never freezes while rg scans; streams stdout (no maxBuffer cap).
function contentSearch(term) {
  return new Promise((resolve) => {
    if (!term || !RG) return resolve(null);
    let buf = '';
    let child;
    try { child = spawn(RG, ['-l', '-i', '-F', '--glob', '*.jsonl', '--', term, ROOT]); }
    catch { return resolve(null); }
    child.stdout.on('data', (d) => { buf += d; });
    child.on('error', () => resolve(null));
    child.on('close', () => resolve({ query: term, ids: new Set(buf.split('\n').filter(Boolean).map((f) => path.basename(f, '.jsonl'))) }));
  });
}

function tabBar() {
  const cols = out.columns || 80;
  const lines = []; let cur2 = '', w = 0;
  projects.forEach((p, i) => {
    const vis = p.length + 2;
    if (w + vis > cols && cur2) { lines.push(cur2); cur2 = ''; w = 0; }
    cur2 += (i === pIdx ? `${INV} ${p} ${R}` : `${D} ${p} ${R}`);
    w += vis;
  });
  if (cur2) lines.push(cur2);
  return lines;
}

function preview(it, width, replyMax = 6) {
  if (!it) return [];
  const w = Math.min(width, out.columns || 80);
  const rule = D + '─'.repeat(w) + R;
  const kind = it.custom ? `${YE}named${R}` : it.ai ? `${D}auto-named${R}` : `${D}unnamed${R}`;
  const lines = [rule];
  lines.push(`${CY}${it.name}${R}  ${kind}`);
  const prompts = it._loaded ? ` · ${it.count} prompt${it.count === 1 ? '' : 's'}` : ''; // count needs the lazy read
  lines.push(`${D}${it.project} · ${ago(it.mtime)} ago${prompts} · ${sizeFmt(it.size)}${it.branch ? ' · ' + it.branch : ''}${R}`);
  if (it.open) lines.push(`${YE}▸ pick up${R}${D} · ${it.openWhy || 'unfinished'}${R}`);
  if (archived.has(it.id)) lines.push(`${D}🗄 archived · hidden from other tabs · ^a to unarchive${R}`);
  if (deep) lines.push(`${YE}✓ contains "${deep.query}"${R}`);
  const rel = (iso) => iso ? ` [${ago(new Date(iso).getTime())}]` : '';
  const quote = (l) => `${D}│${R} ${l}`; // blockquote gutter marks rendered-markdown blocks apart from plain prompts
  if (it.summary) {
    lines.push(`${D}summary${it.summaryTs ? ` · ${ts(it.summaryTs).trim()}${rel(it.summaryTs)}` : ''}${R}`);
    mdLines(it.summary, w - 2).slice(0, 4).forEach((l) => lines.push(quote(l)));
  }
  lines.push(`${D}first · ${ts(it.firstTs).trim()}${rel(it.firstTs)}${R}`);
  wrap(it.first, w, 2).forEach((l) => lines.push(l));
  if (!it._loaded) lines.push(`${D}…${R}`); // detail (last/reply/summary) still loading
  if (it.count > 1) {
    lines.push(`${D}last · ${ts(it.lastTs).trim()}${rel(it.lastTs)}${R}`);
    wrap(it.last, w, 2).forEach((l) => lines.push(l));
  }
  if (it.reply && replyMax > 0) { // Claude's last response — where the session ended up
    lines.push(`${V}reply${R}${D} · ${ts(it.replyTs).trim()}${rel(it.replyTs)}${R}`);
    const rl = mdLines(it.reply, w - 2);
    rl.slice(0, replyMax).forEach((l) => lines.push(quote(l)));
    if (rl.length > replyMax) lines.push(quote(`${D}… ⇥ for full${R}`)); // more below the cut
  }
  return lines;
}

function draw() {
  const list = view();
  if (cur >= list.length) cur = Math.max(0, list.length - 1);
  const tabs = tabBar();
  const cols = out.columns || 80;
  const rows = out.rows || 24;
  // reply is capped so the preview never scrolls the frame off; ^e expands it to fill,
  // keeping at least MIN_LIST rows of list. default is 6 reply lines.
  // default keeps a healthy list and gives the reply the rest (shows short/medium replies in full);
  // ^e/⇥ expand shrinks the list to MIN_LIST so long replies get maximum room.
  const MIN_LIST = 3, DEFAULT_LIST = 6;
  const base = preview(list[cur], cols, 0).length; // preview height without the reply block
  const budget = rows - (1 + tabs.length + 1 + 1) - base - 2 - (expand ? MIN_LIST : DEFAULT_LIST);
  const replyMax = Math.max(1, budget);
  const prev = preview(list[cur], cols, replyMax);
  // reserve space for header, tabs, query, preview, and the show-more line, so
  // the viewport (rows visible at once) never pushes anything off-screen.
  const overhead = 1 + tabs.length + 1 + prev.length + 1;
  const maxFit = Math.max(3, (out.rows || 24) - overhead);
  if (limit > maxFit) limit = maxFit;                 // cap growth to what fits
  // never hide an ACTIVE (green) session behind "show more"; recent/orange stay top-sorted + paged
  const activeCount = list.filter((i) => Date.now() - i.mtime < ACTIVE_MS).length;
  const win = Math.min(Math.max(limit, activeCount), maxFit, list.length);
  if (cur < off) off = cur;
  if (cur >= off + win) off = cur - win + 1;
  if (off < 0) off = 0;
  const hint = RG ? `^f search-in-text · ` : '';
  const arch = projects[pIdx] === ARCHIVED_TAB ? `^a unarchive · ` : `^a archive · `;
  const exp = expand ? `${CY}⇥ collapse${D} · ` : `⇥ expand-reply · `;
  let s = CLR + `${D}←→ project · ↑↓ move · type · ${hint}${arch}${exp}↵ resume · esc quit · ${CY}live${D}${R}\n`;
  s += tabs.join('\n') + '\n';
  const prompt = deep ? `${YE}content›${R}` : `${CY}›${R}`;
  s += `${prompt} ${q}${D}▏${R}${deep ? `  ${YE}${list.length} match${list.length === 1 ? '' : 'es'}${R}` : ''}\n`;
  const slice = list.slice(off, off + win);
  if (!slice.length) s += `${D}  no match${R}\n`;
  slice.forEach((it, i) => {
    const on = off + i === cur;
    const nm = it.name.slice(0, 50).padEnd(50);
    const showProj = projects[pIdx] === 'All' || projects[pIdx] === OPEN_TAB;
    const meta = (showProj ? it.project.slice(0, 16).padEnd(16) + ' ' : '') + sizeFmt(it.size).padStart(5) + ' ' + ago(it.mtime).padStart(3);
    const age = Date.now() - it.mtime; // col0 recency dot: green<5m, orange<24h
    const dot = age < ACTIVE_MS ? `${G}●${R}` : age < RECENT_MS ? `${O}●${R}` : ' ';
    const om = it.open ? `${YE}▸${R}` : ' '; // col1 open marker: pick up where you left off
    if (on) { const bar = `${nm}  ${meta} `.slice(0, cols - 2).padEnd(cols - 2); s += `${dot}${om}${INV}${bar}${R}\n`; }
    else s += `${dot}${om}${nm}  ${D}${meta}${R}\n`;
  });
  const below = list.length - (off + win);
  if (below > 0) s += `${D} ↓ ${below} more — press ↓ to reveal${R}\n`; // attached to the list
  s += prev.join('\n') + '\n';                                          // then the session details
  out.write(s);
}

readline.emitKeypressEvents(process.stdin);
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.resume();
out.write(HIDE);
draw();
ensureDetail();

// live refresh: rescan every 2s, preserving filter, active tab, highlighted row.
let refreshing = false;
const timer = setInterval(async () => {
  if (refreshing) return;
  refreshing = true;
  try {
    const selId = view()[cur]?.id;
    const activeName = projects[pIdx];
    items = await load();
    projects = tabsFor(items);
    const np = projects.indexOf(activeName);
    pIdx = np >= 0 ? np : 0;
    const v = view();
    const ni = selId ? v.findIndex((i) => i.id === selId) : -1;
    cur = ni >= 0 ? ni : Math.min(cur, Math.max(0, v.length - 1));
    draw();
    ensureDetail();
  } finally { refreshing = false; }
}, 2000);
timer.unref?.();

// never leave the terminal in raw mode / cursor hidden, and surface the real error
function restore() { try { if (process.stdin.isTTY) process.stdin.setRawMode(false); } catch {} out.write(SHOW); }
process.on('uncaughtException', (e) => { try { clearInterval(timer); } catch {} restore(); out.write(CLR); console.error('sessions error:', (e && e.stack) || e); process.exit(1); });
process.on('SIGINT', () => { clearInterval(timer); restore(); out.write(CLR); process.exit(0); });

process.stdin.on('keypress', (str, key) => {
  const list = view();
  if (key.ctrl && key.name === 'c') { clearInterval(timer); out.write(CLR); process.exit(0); }
  else if (key.name === 'escape') {
    if (deep) { deep = null; searchGen++; cur = 0; off = 0; limit = 12; draw(); ensureDetail(); } // first esc clears content search
    else { clearInterval(timer); out.write(CLR); process.exit(0); }  // second esc quits
  }
  else if (key.ctrl && key.name === 'f') {                     // run content search on current query (async)
    if (RG && q) {
      const gen = ++searchGen, term = q;
      out.write(`${CY}searching…${R}\r`);
      contentSearch(term).then((res) => {
        if (gen !== searchGen) return; // a newer query/search superseded this one
        deep = res; cur = 0; off = 0; limit = 12; draw(); ensureDetail();
      });
    }
  }
  else if (key.ctrl && key.name === 'a') {                     // archive/unarchive: sessio-local hide only
    const p = list[cur]; if (!p) return;
    if (archived.has(p.id)) archived.delete(p.id); else archived.add(p.id);
    saveArchived(archived);
    const activeName = projects[pIdx];
    projects = tabsFor(items);                                // archived tab / emptied project tabs may appear/disappear
    const np = projects.indexOf(activeName);
    pIdx = np >= 0 ? np : 0;                                   // fall back to All if the current tab vanished
    cur = Math.min(cur, Math.max(0, view().length - 1)); off = 0;
    draw(); ensureDetail();
  }
  else if (key.name === 'tab' || (key.ctrl && key.name === 'e')) { expand = !expand; draw(); } // expand/collapse the reply
  else if (key.name === 'up') { cur = Math.max(0, cur - 1); draw(); ensureDetail(); }
  else if (key.name === 'down') { if (cur < list.length - 1) { cur++; if (cur >= limit) limit += 12; } draw(); ensureDetail(); } // reveal more
  else if (key.name === 'left') { pIdx = (pIdx - 1 + projects.length) % projects.length; cur = 0; off = 0; limit = 12; draw(); ensureDetail(); }
  else if (key.name === 'right') { pIdx = (pIdx + 1) % projects.length; cur = 0; off = 0; limit = 12; draw(); ensureDetail(); }
  else if (key.name === 'backspace') { q = q.slice(0, -1); deep = null; searchGen++; cur = 0; off = 0; limit = 12; draw(); ensureDetail(); }
  else if (key.name === 'return') {
    const p = list[cur]; if (!p) return;
    clearInterval(timer);
    if (process.stdin.isTTY) process.stdin.setRawMode(false);
    out.write(CLR + SHOW); // restore cursor before handing the terminal to claude
    const cmd = `cd ${p.cwd || '.'} && claude --resume ${p.id}`;
    if (!p.cwd) { console.log(cmd); process.exit(0); }
    try {
      const r = spawnSync('claude', ['--resume', p.id], { cwd: p.cwd, stdio: 'inherit' });
      if (r.error) throw r.error; // e.g. claude not on PATH, or nested-launch failure
      process.exit(r.status ?? 0);
    } catch (e) {
      restore();
      console.log(`\nCouldn't launch claude (${e.code || e.message}). Run it yourself:\n  ${cmd}\n`);
      process.exit(1);
    }
  }
  else if (str && !key.ctrl && !key.meta && str.length === 1 && str >= ' ') { q += str; deep = null; searchGen++; cur = 0; off = 0; limit = 12; draw(); ensureDetail(); }
});
