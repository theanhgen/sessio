#!/usr/bin/env node
// `sessions` — pick a past Claude session and resume it.
// ←→ project · ↑↓ move · type to filter · ↵ resume · esc quit. Live-refreshes.

import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import readline from 'node:readline';
import https from 'node:https';
import { fileURLToPath } from 'node:url';
import { spawnSync, spawn, execFile } from 'node:child_process';
import { sanitizeTerminalText } from '../lib/safety.mjs';
import { cacheKey, pruneCache, selectTranscriptRows } from '../lib/session-store.mjs';

const ROOT = path.join(os.homedir(), '.claude', 'projects');
const CAP = 300; // scan the 300 most-recent sessions; plenty for getting back into recent work

// archive is a sessio-local hide list only: ids live in ~/.claude/.sessio/archived.json.
// the transcript files are never touched, so `claude --resume` still works and Claude's own
// cleanupPeriodDays still applies — this declutters the list, it does not preserve sessions.
const SESSIO_DIR = path.join(os.homedir(), '.claude', '.sessio');
const ARCHIVE_FILE = path.join(SESSIO_DIR, 'archived.json');
// persistent list-metadata cache: relative transcript path -> {mtime, ...head, ...tail}. Keyed by mtime so a
// changed transcript is re-read; survives across launches so a cold start is near-instant.
const HCACHE_FILE = path.join(SESSIO_DIR, 'list-cache.json');
// Entries are `{k, at}` since sessio started stamping when a session was archived (so one that
// is written to afterwards can come back out on its own). Bare strings are the older shape and
// still load — this reference implementation only needs to read what the Rust build writes.
const loadArchived = () => {
  try {
    const raw = JSON.parse(fs.readFileSync(ARCHIVE_FILE, 'utf8'));
    return new Set(raw.map((e) => (e && typeof e === 'object' ? e.k : e)).filter((k) => typeof k === 'string'));
  } catch { return new Set(); }
};
function savePrivateJson(file, value) {
  const tmp = `${file}.${process.pid}.tmp`;
  try {
    fs.mkdirSync(SESSIO_DIR, { recursive: true, mode: 0o700 });
    fs.writeFileSync(tmp, JSON.stringify(value), { mode: 0o600 });
    fs.renameSync(tmp, file);
    fs.chmodSync(file, 0o600);
    return true;
  } catch {
    try { fs.unlinkSync(tmp); } catch {}
    return false;
  }
}
const saveArchived = (set) => savePrivateJson(ARCHIVE_FILE, [...set]);
let archived = loadArchived();
const archiveKey = (item) => item.key || item.id;
const isArchived = (item) => archived.has(archiveKey(item)) || archived.has(item.id); // supports pre-0.3.1 id-only archives

const safeText = sanitizeTerminalText;
const validPath = (value) => typeof value === 'string' && safeText(value) === value ? value : null;

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
// Which `promptSource` values count as something a human actually asked. Gating on 'typed'
// alone hid every background session (the task panel records its prompt as 'queued'), because a
// session with no first prompt is dropped from the list entirely. 'system' is the one source
// that is not the user speaking.
const HUMAN_PROMPT = new Set(['typed', 'queued', 'suggestion_accepted']);
// list-cache.json keys parsed head/tail info by mtime alone, so a change to what head() extracts
// keeps serving the old answer forever on files that never change again. Bump on every parser
// change: entries stamped with an older version are re-read.
const HEAD_V = 3;
function head(file) {
  return new Promise((res) => {
    const rl = readline.createInterface({ input: fs.createReadStream(file) });
    let n = 0, first = null, firstTs = null, cwd = null, branch = null, custom = null, ai = null;
    rl.on('line', (l) => {
      n++;
      if (l.includes('-title"')) { try { const o = JSON.parse(l); if (o.type === 'custom-title' && o.customTitle) custom = safeText(o.customTitle); else if (o.type === 'ai-title' && o.aiTitle) ai = ai || safeText(o.aiTitle); } catch {} }
      else if (l.includes('"promptSource":"')) {
        try { const o = JSON.parse(l); if (o.type === 'user' && o.message && HUMAN_PROMPT.has(o.promptSource)) { const c = o.message.content; const text = typeof c === 'string' ? safeText(c) : ''; if (text.trim() && !text.startsWith('<')) { if (!first) { first = text.trim(); firstTs = o.timestamp || null; } if (!cwd && o.cwd) cwd = validPath(o.cwd); if (!branch && o.gitBranch) branch = safeText(o.gitBranch); } } } catch {}
      }
      else if (!cwd && l.includes('"cwd":"')) { try { const o = JSON.parse(l); if (o.cwd) cwd = validPath(o.cwd); } catch {} }
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
    let cwd = null, first = null, last = null, firstTs = null, lastTs = null, count = 0, custom = null, ai = null, branch = null, summary = null, summaryTs = null, reply = null, replyTs = null, recap = null, recapTs = null;
    rl.on('line', (l) => {
      if (l.includes('-title"')) {
        try { const o = JSON.parse(l); if (o.type === 'custom-title' && o.customTitle) custom = safeText(o.customTitle); else if (o.type === 'ai-title' && o.aiTitle) ai = safeText(o.aiTitle); } catch {}
        return;
      }
      if (l.includes('"away_summary"')) { // Claude's away-recap; the last one in the file wins
        try { const o = JSON.parse(l); if (o.type === 'system' && o.subtype === 'away_summary') { const t = cleanRecap(o.content); if (t) { recap = t; recapTs = o.timestamp || null; } } } catch {}
        return;
      }
      if (l.includes('"isCompactSummary":true')) { // auto-compact recap; keep the latest, strip boilerplate
        try {
          const o = JSON.parse(l);
          if (o.isCompactSummary === true) { // exact field, not a mention of the word
            const c = o.message && o.message.content; // string in some sessions, array of blocks in others
            const t = typeof c === 'string' ? c : Array.isArray(c) ? c.map((x) => (x && x.text) || '').join('\n\n') : '';
            if (t) { const i = t.indexOf('Summary:'); summary = safeText(i >= 0 ? t.slice(i + 8) : t).trim(); summaryTs = o.timestamp || null; }
          }
        } catch {}
        return;
      }
      if (l.includes('"type":"assistant"') && l.includes('"type":"text"')) { // keep the latest assistant text reply
        try {
          const o = JSON.parse(l);
          if (o.type === 'assistant' && o.message && Array.isArray(o.message.content)) {
            const txt = o.message.content.filter((b) => b.type === 'text').map((b) => b.text).join('\n\n');
            const text = safeText(txt);
            if (text.trim()) { reply = text.trim(); replyTs = o.timestamp || null; }
            if (!cwd && o.cwd) cwd = validPath(o.cwd);
          }
        } catch {}
        return;
      }
      if (l.includes('"promptSource":"')) {
        try {
          const o = JSON.parse(l);
          if (o.type === 'user' && o.message && HUMAN_PROMPT.has(o.promptSource)) {
            const c = o.message.content;
            const text = typeof c === 'string' ? safeText(c) : '';
            if (text.trim() && !text.startsWith('<')) {
              count++;
              if (!first) { first = text.trim(); firstTs = o.timestamp || null; }
              last = text.trim(); lastTs = o.timestamp || null;
              if (!cwd && o.cwd) cwd = validPath(o.cwd);
              if (!branch && o.gitBranch) branch = safeText(o.gitBranch);
            }
          }
        } catch {}
        return;
      }
      if (!cwd && l.includes('"cwd":"')) { try { const o = JSON.parse(l); if (o.cwd) cwd = validPath(o.cwd); } catch {} }
    });
    const done = () => res({ cwd, first, last, firstTs, lastTs, count, custom, ai, branch, summary, summaryTs, reply, replyTs, recap, recapTs, _loaded: true });
    rl.on('close', done); rl.on('error', done);
  });
}

// cheap tail read (last ~64KB) to tell whether a session is "open": who spoke last,
// and — if Claude — whether it ended on a question / proposal you didn't answer.
const CTA = /\?\s*["')\]]*\s*$|\b(want me to|should i|shall i|do you want|let me know|ready to|next step|proceed\b|which (one|of)|confirm)\b/i;
// Why a session counts as "open". These strings are user-visible in the preview AND the 3-day
// decay below matches on one of them, so they are defined exactly once: the decay previously
// compared against 'Claude proposed next' while tail() wrote 'Claude asked / proposed next',
// which silently disabled it for the whole life of the feature.
const WHY_UNANSWERED = 'your prompt got no reply';
const WHY_RECAP = 'recap says your move';
const WHY_CTA = 'Claude asked / proposed next';
const WHY_GIT = 'uncommitted changes';
// Claude writes an away-recap when you leave a session: `{type:'system', subtype:'away_summary'}`
// with a goal / state / whose-move paragraph in `content`. It is worth far more than the CTA
// guess below — it *states* who owes the next move instead of pattern-matching a question mark —
// and it is present in far more transcripts than the compact summary the preview used before.
const RECAP_OPEN = /next action is yours/i;
/** Strip the fixed UI hint Claude appends; it is noise in a preview. */
const cleanRecap = (c) => safeText(typeof c === 'string' ? c : '')
  .replace(/\s*\(disable recaps in \/config\)\s*$/, '')
  .trim();
function tail(file) {
  return new Promise((res) => {
    fs.stat(file, (e, st) => {
      if (e) return res({});
      const rl = readline.createInterface({ input: fs.createReadStream(file, { start: Math.max(0, st.size - 65536) }) });
      let reply = null, replyTs = null, userTs = null; // last assistant text vs last typed user prompt
      let recap = null, recapTs = null;                 // Claude's latest away-recap
      rl.on('line', (l) => {
        if (l.includes('"away_summary"')) {
          try { const o = JSON.parse(l); if (o.type === 'system' && o.subtype === 'away_summary') { const t = cleanRecap(o.content); if (t) { recap = t; recapTs = o.timestamp || recapTs; } } } catch {}
        } else if (l.includes('"type":"assistant"') && l.includes('"type":"text"')) {
          try { const o = JSON.parse(l); if (o.type === 'assistant' && Array.isArray(o.message?.content)) { const t = safeText(o.message.content.filter((b) => b.type === 'text').map((b) => b.text).join('\n\n')); if (t.trim()) { reply = t.trim(); replyTs = o.timestamp || replyTs; } } } catch {}
        } else if (l.includes('"promptSource":"')) {
          try { const o = JSON.parse(l); const text = typeof o.message?.content === 'string' ? safeText(o.message.content) : ''; if (o.type === 'user' && HUMAN_PROMPT.has(o.promptSource) && text.trim() && !text.startsWith('<')) userTs = o.timestamp || userTs; } catch {}
        }
      });
      const done = () => {
        const T = (x) => x ? new Date(x).getTime() : 0;
        let open = false, why = '';
        if (userTs && T(userTs) > T(replyTs)) { open = true; why = WHY_UNANSWERED; }
        else if (recap && RECAP_OPEN.test(recap) && T(recapTs) >= T(replyTs)) { open = true; why = WHY_RECAP; }
        else if (reply && CTA.test(reply.slice(-400))) { open = true; why = WHY_CTA; }
        res({ open, why, recap, recapTs });
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
  if (!fs.existsSync(cwd)) { gitCache.set(cwd, { dirty: false, at: Date.now() }); return; } // dir gone: cache clean, don't spawn
  gitInflight.add(cwd);
  // NB: `git status` (not a bare `.git` check) so sessions started in a repo subdir are still detected.
  execFile('git', ['-C', cwd, 'status', '--porcelain'], { encoding: 'utf8', timeout: 2000, maxBuffer: 1 << 20 }, (err, stdout) => {
    gitInflight.delete(cwd);
    gitCache.set(cwd, { dirty: !err && stdout.trim().length > 0, at: Date.now() });
  });
}
// return the best-known dirtiness immediately; refresh in the background when stale/unknown.
function gitDirty(cwd) {
  const c = gitCache.get(cwd);
  if (!c || Date.now() - c.at >= 30000) gitRefresh(cwd); // recheck at most every 30s (WIP doesn't change fast)
  return c ? c.dirty : false; // unknown until the first background check resolves
}

function wrap(text, width, maxLines) {
  const words = safeText(text).replace(/\s+/g, ' ').trim().split(' ');
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

// run fn over items with bounded concurrency — parallel enough to be fast, capped so a big
// scan can't exhaust file descriptors (macOS defaults to a 256 fd limit).
async function mapLimit(items, limit, fn) {
  let i = 0;
  const worker = async () => { while (i < items.length) { const idx = i++; await fn(items[idx], idx); } };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
}

// list metadata cache, seeded from disk so repeat launches skip re-parsing unchanged transcripts.
const hCache = (() => {
  try { return new Map(Object.entries(JSON.parse(fs.readFileSync(HCACHE_FILE, 'utf8')))); } catch { return new Map(); }
})();
const dCache = new Map(); // transcript path -> {mtime, ...detail()} preview metadata (lazy, per highlight)
let hCacheDirty = false;   // only rewrite the disk cache when a miss actually changed it
// Persist a private, atomic cache and prune any deleted or aged-out transcript metadata.
function saveHCache(ids) {
  const keys = new Set(ids);
  const pruned = pruneCache(hCache, keys);
  const needsPersist = hCacheDirty || pruned.size !== hCache.size;
  if (!needsPersist) return;
  hCache.clear();
  for (const [key, value] of pruned) hCache.set(key, value);
  if (savePrivateJson(HCACHE_FILE, Object.fromEntries(pruned))) hCacheDirty = false;
}
async function load(extraFiles = new Set()) {
  const rows = [];
  let dirs;
  try { dirs = fs.readdirSync(ROOT); } catch (e) { if (e && e.code === 'ENOENT') { saveHCache([]); return []; } throw e; }
  for (const d of dirs) {
    const dir = path.join(ROOT, d);
    let st; try { st = fs.statSync(dir); } catch { continue; }
    if (!st.isDirectory()) continue;
    for (const f of fs.readdirSync(dir)) {
      if (!f.endsWith('.jsonl')) continue;
      let s; try { s = fs.statSync(path.join(dir, f)); } catch { continue; }
      if (s.isFile()) {
        const file = path.join(dir, f);
        rows.push({ id: f.slice(0, -6), key: cacheKey(ROOT, file), file, mtime: s.mtimeMs, size: s.size, dir: d });
      }
    }
  }
  rows.sort((a, b) => b.mtime - a.mtime);
  const slice = selectTranscriptRows(rows, CAP, extraFiles);
  // read head+tail for all rows concurrently, reusing the cache when mtime is unchanged.
  await mapLimit(slice, 48, async (r) => {
    let info = hCache.get(r.key);
    if (!info || info.mtime !== r.mtime || info.v !== HEAD_V) {
      info = { v: HEAD_V, mtime: r.mtime, ...(await head(r.file)), ...(await tail(r.file)) };
      hCacheDirty = true;
    }
    hCache.set(r.key, info);
    Object.assign(r, info);
  });
  const items = [];
  for (const r of slice) {
    if (!r.first) continue; // skip empty (0-prompt) sessions
    r.title = r.custom || r.ai || null;
    r.name = r.title || r.first;
    // "Claude proposed next" is weak (most replies offer next steps) — only count it when
    // recent (last 3 days); unanswered prompts and git-WIP count at any age.
    if (r.open && r.why === WHY_CTA && Date.now() - r.mtime > 3 * 86400000) r.open = false;
    if (r.open) r.openWhy = r.why;
    const d = dCache.get(r.key); // fold in detail if it's already been read for this session
    if (d && d.mtime === r.mtime) Object.assign(r, d);
    items.push(r);
  }
  // git WIP: flag the most-recent session in each project whose folder has uncommitted changes
  const seenCwd = new Set();
  for (const r of items) {
    if (!r.cwd || seenCwd.has(r.cwd)) continue;
    seenCwd.add(r.cwd);
    if (gitDirty(r.cwd)) { r.open = true; r.openWhy = r.openWhy || WHY_GIT; }
  }
  // one canonical label per project dir: prefer a sibling's real cwd basename (hyphens
  // intact); fall back to the dash-decoded name only if no session in the dir has a cwd.
  const dirLabel = new Map();
  for (const r of items) if (r.cwd && !dirLabel.has(r.dir)) dirLabel.set(r.dir, path.basename(r.cwd));
  for (const r of items) {
    r.project = safeText(dirLabel.get(r.dir) || proj(r.dir));
    r.hay = (r.project + ' ' + r.name + ' ' + r.first).toLowerCase();
  }
  saveHCache(slice.map((r) => r.key));
  return items;
}

// --- explicit update (sessions --update) ---
// Starting a session browser must never modify its checkout or global installation. Updates are
// deliberately opt-in and never run `git pull` in a developer checkout.
const PKG_ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), '..');
const readVersion = (root) => { try { return JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8')); } catch { return null; } };
const isNewer = (a, b) => { const pa = a.split('.').map(Number), pb = b.split('.').map(Number); for (let i = 0; i < 3; i++) { const x = pa[i] || 0, y = pb[i] || 0; if (x !== y) return x > y; } return false; };
const writable = (p) => { try { fs.accessSync(p, fs.constants.W_OK); return true; } catch { return false; } };
function latestVersion(name, timeoutMs) {
  return new Promise((resolve) => {
    // full packument (abbreviated form) — the /<name>/latest endpoint returns empty on npm, so
    // read dist-tags.latest from the package doc instead.
    const req = https.get(`https://registry.npmjs.org/${name}`, { timeout: timeoutMs, headers: { accept: 'application/vnd.npm.install-v1+json' } }, (res) => {
      if (res.statusCode !== 200) { res.resume(); return resolve(null); }
      res.on('error', () => resolve(null)); // a mid-transfer connection drop would otherwise throw uncaught
      let d = ''; res.on('data', (c) => { d += c; }); res.on('end', () => { try { const o = JSON.parse(d); resolve((o['dist-tags'] && o['dist-tags'].latest) || null); } catch { resolve(null); } });
    });
    req.on('timeout', () => { req.destroy(); resolve(null); });
    req.on('error', () => resolve(null));
  });
}
async function update() {
  if (process.env.NO_UPDATE_NOTIFIER || process.env.SESSIO_NO_UPDATE) {
    console.log('Updates are disabled by NO_UPDATE_NOTIFIER or SESSIO_NO_UPDATE.');
    return;
  }
  const pkg = readVersion(PKG_ROOT);
  if (!pkg || !pkg.name || !pkg.version) return;
  const cur = pkg.version;
  const latest = await latestVersion(pkg.name, 2000);         // bounded: slow/offline network never delays launch
  if (!latest) { console.error('Could not check npm for a newer sessio version.'); process.exitCode = 1; return; }
  if (!isNewer(latest, cur)) { console.log(`sessio ${cur} is current.`); return; }
  const isGit = fs.existsSync(path.join(PKG_ROOT, '.git'));
  if (isGit) {
    process.stdout.write(`sessio ${latest} available — update this checkout yourself:\n  git -C ${PKG_ROOT} pull\n`);
    return;
  }
  const gRoot = (() => { try { return spawnSync('npm', ['root', '-g'], { encoding: 'utf8', timeout: 5000 }).stdout.trim(); } catch { return ''; } })();
  if (!gRoot || !path.resolve(PKG_ROOT).startsWith(path.resolve(gRoot))) {
    process.stdout.write(`sessio ${latest} available (you have ${cur}) — run: npm i -g ${pkg.name}\n`);
    return;
  }
  if (!writable(PKG_ROOT)) { // a global install owned by root — don't trigger a sudo prompt/hang
    process.stdout.write(`sessio ${latest} available (you have ${cur}) — run: sudo npm i -g ${pkg.name}\n`);
    return;
  }
  process.stdout.write(`updating sessio ${cur} → ${latest}…\n`);
  const r = spawnSync('npm', ['i', '-g', `${pkg.name}@latest`], { stdio: 'ignore', timeout: 120000 });
  if (r.status === 0) { process.stdout.write(`Updated sessio to ${latest}.\n`); return; }
  process.exitCode = 1;
  process.stdout.write(`sessio ${latest} available — couldn't update; run: npm i -g ${pkg.name}\n`);
}
if (process.argv.slice(2).includes('--update')) {
  try { await update(); } catch { console.error('Could not update sessio.'); process.exitCode = 1; }
  process.exit(process.exitCode || 0);
}

// Hidden: dump what load() computed, as stable JSON, and exit. This is the oracle the Rust
// port is diffed against — same transcripts in, byte-identical JSON out, or the port is wrong.
// Deliberately excludes absolute paths (machine-specific) and rounds mtime to whole ms, since
// stat precision and float formatting differ across platforms and runtimes.
if (process.argv.slice(2).includes('--dump-json')) {
  const rows = await load();
  const dump = rows.map((r) => ({
    key: r.key, id: r.id, dir: r.dir, project: r.project,
    mtime: Math.round(r.mtime), size: r.size,
    name: r.name ?? null, title: r.title ?? null, custom: r.custom ?? null, ai: r.ai ?? null,
    first: r.first ?? null, firstTs: r.firstTs ?? null,
    cwd: r.cwd ?? null, branch: r.branch ?? null,
    open: !!r.open, openWhy: r.openWhy ?? null,
    hay: r.hay ?? null,
  }));
  // Wait for the flush callback before exiting: stdout writes to a pipe are asynchronous, and
  // process.exit() would truncate the dump at the 64KB pipe buffer.
  await new Promise((resolve) => process.stdout.write(JSON.stringify(dump) + '\n', resolve));
  process.exit(0);
}

process.stdout.write('loading…\r');
let items = await load();
if (!items.length) { console.log('No sessions found.'); process.exit(0); }
if (!process.stdin.isTTY || !process.stdout.isTTY) {
  console.error('sessions requires an interactive terminal.');
  process.exit(1);
}

// --- picker ---
const out = process.stdout;
const D = '\x1b[2m', CY = '\x1b[36m', YE = '\x1b[33m', G = '\x1b[32m', O = '\x1b[38;5;208m', V = '\x1b[38;5;141m', B = '\x1b[1m', INV = '\x1b[7m', R = '\x1b[0m', CLR = '\x1b[2J\x1b[H';
const ACTIVE_MS = 5 * 60 * 1000;        // green dot: active (written in last 5 min)
const RECENT_MS = 24 * 60 * 60 * 1000;  // orange dot: recent (last 24h, but not active)
const HIDE = '\x1b[?25l', SHOW = '\x1b[?25h'; // hide/show the terminal cursor
process.on('exit', () => out.write(SHOW)); // always restore cursor, whatever the exit path

// minimal markdown -> ANSI renderer so replies/summaries look like Claude renders them
const stripAnsi = (s) => s.replace(/\x1b\[[0-9;]*m/g, '');
const inlineMd = (s) => safeText(s)
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
/** Truncate a styled line to `cols` visible columns. ANSI codes are zero-width, so a naive
 *  slice() would cut mid-escape and bleed styling into the rest of the frame. */
function clipLine(s, cols) {
  if (cols <= 0) return '';
  if (stripAnsi(s).length <= cols) return s;
  let outp = '', vis = 0;
  for (let i = 0; i < s.length; ) {
    if (s[i] === '\x1b') {
      const m = /^\x1b\[[0-9;]*m/.exec(s.slice(i));
      if (m) { outp += m[0]; i += m[0].length; continue; }   // copy the escape, costs no columns
    }
    if (vis >= cols) break;
    outp += s[i]; vis++; i++;
  }
  return outp + R;
}
/** Fit a whole frame to the viewport.
 *
 *  Every line must fit `cols` and the frame must fit `rows`, because CLR is `\x1b[2J\x1b[H` —
 *  it clears the *visible* screen only. If any line wraps, the frame grows past `rows`, the
 *  terminal scrolls, and the rows that scrolled off land in the scrollback where the next CLR
 *  can't reach them. The redraw then stacks a fresh copy of the UI under every previous one
 *  instead of repainting in place. No trailing newline for the same reason: a newline on the
 *  last row scrolls by one. */
function fitFrame(lines, cols, rows) {
  return lines.slice(0, Math.max(1, rows)).map((l) => clipLine(l, cols)).join('\n');
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
  width = Math.max(1, width | 0); // guard: a 1-2 col terminal can pass width<=0; the code hard-wrap below would loop forever
  const res = [];
  const raw = safeText(text).split('\n');
  const isRow = (s) => /^\s*\|.*\|\s*$/.test(s);
  const isSep = (s) => s.includes('|') && /-/.test(s) && /^\s*\|?[\s:|-]+\|?\s*$/.test(s);
  const cells = (s) => s.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map((c) => c.trim());
  for (let i = 0; i < raw.length; i++) {
    let line = raw[i].replace(/\s+$/, '');
    if (/^\s*```/.test(line)) { // fenced code: render verbatim (no inlineMd, so ** / ` inside code aren't mangled)
      let j = i + 1;
      while (j < raw.length && !/^\s*```/.test(raw[j])) {
        let code = raw[j].replace(/\t/g, '  ').replace(/\s+$/, '');
        if (!code) res.push('');
        else while (code.length) { res.push(`${D}${code.slice(0, width)}${R}`); code = code.slice(width); } // hard-wrap wide lines
        j++;
      }
      i = j; // skip the closing ``` (or run to end if unterminated)
      continue;
    }
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
let help = false;  // full-screen keybinding overlay (toggled by ?)
let flash = '';    // transient one-line notice (e.g. "opened in a new window"); any key dismisses it
const OPEN_TAB = '⏸ open';
const ARCHIVED_TAB = '🗄 archived';
// tabs are built from live (non-archived) sessions; the archived tab appears only while
// something is archived, and always sits last.
const tabsFor = (its) => {
  const live = its.filter((i) => !isArchived(i));
  return ['All', ...(live.some((i) => i.open) ? [OPEN_TAB] : []), ...new Set(live.map((i) => i.project)),
    ...(its.some((i) => isArchived(i)) ? [ARCHIVED_TAB] : [])];
};
let projects = tabsFor(items);
// subsequence fuzzy match + score: a contiguous substring always outranks a scattered match;
// within each, earlier position and word-boundary / streak hits score higher. Returns -1 when
// the needle isn't a subsequence of hay (hay is already lowercased; needle is lowercased here).
function fuzzyScore(hay, needleRaw) {
  const needle = needleRaw.toLowerCase();
  if (!needle) return 0;
  const idx = hay.indexOf(needle);
  if (idx >= 0) return 10000 - idx;                          // substring: strong, rank by earliness
  let i = 0, score = 0, streak = 0, prev = -2;
  for (let c = 0; c < hay.length && i < needle.length; c++) {
    if (hay[c] === needle[i]) {
      streak = c === prev + 1 ? streak + 1 : 0;
      score += 1 + streak;
      if (c === 0 || /[\s/_.\-]/.test(hay[c - 1])) score += 3; // word-boundary bonus
      prev = c; i++;
    }
  }
  return i === needle.length ? score : -1;                    // -1 => not a subsequence, filtered out
}
const view = () => {
  const p = projects[pIdx];
  let l;
  if (p === ARCHIVED_TAB) l = items.filter((i) => isArchived(i));      // archived tab: only archived
  else {
    l = p === 'All' ? items : p === OPEN_TAB ? items.filter((i) => i.open) : items.filter((i) => i.project === p);
    l = l.filter((i) => !isArchived(i));                              // every other tab: hide archived
  }
  if (deep) l = l.filter((i) => deep.keys.has(i.key));      // content match wins
  else if (q) {
    // substring-first: show sessions that literally contain the query (name/project/first prompt),
    // ranked earliest-hit then most-recent. Only if nothing contains it do we fall back to the
    // looser subsequence fuzzy match — otherwise a short query like "csu" sprays across prose.
    const nq = q.toLowerCase();
    const subs = [];
    for (const it of l) { const idx = it.hay.indexOf(nq); if (idx >= 0) subs.push([it, idx]); }
    if (subs.length) l = subs.sort((a, b) => a[1] - b[1] || b[0].mtime - a[0].mtime).map((e) => e[0]);
    else l = l.map((it) => [it, fuzzyScore(it.hay, q)]) // fuzzy fallback, ranked (recency tiebreak)
      .filter((e) => e[1] >= 0).sort((a, b) => b[1] - a[1] || b[0].mtime - a[0].mtime).map((e) => e[0]);
  }
  return l;
};

// lazily full-read the highlighted session for its preview (count/last/reply/summary/title)
function ensureDetail() {
  const it = view()[cur];
  if (!it || it._loaded) return;
  const c = dCache.get(it.key);
  if (c && c.mtime === it.mtime) { Object.assign(it, c); it.name = (it.custom || it.ai) || it.name; return; }
  detail(it.file).then((d) => {
    const rec = { mtime: it.mtime, ...d };
    dCache.set(it.key, rec);
    Object.assign(it, d);
    if (it.custom || it.ai) { it.title = it.custom || it.ai; it.name = it.title; }
    draw();
  });
}

// content search: grep every transcript body for the term, then load every matching session
// (including matches beyond the normal browse cap).
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
    child.on('close', () => resolve({ query: term, files: new Set(buf.split('\n').filter(Boolean).map((f) => path.resolve(f))) }));
  });
}

/** Index of `q` with its last word removed: trailing separators first, then the word itself.
 *  Separators include the punctuation in paths and titles, so one ⌥⌫ over `mybit/tooling`
 *  leaves `mybit/`. */
function dropWord(q) {
  const trimmed = q.replace(/[\s/\-_.:,]+$/, '');
  const i = trimmed.search(/[\s/\-_.:,](?=[^\s/\-_.:,]*$)/);
  return i === -1 ? 0 : i + 1;
}

/** Every edit to the query invalidates the content search and the scroll position. */
function requery() {
  deep = null; searchGen++; cur = 0; off = 0; limit = 12;
  draw(); ensureDetail();
}

function tabBar() {
  const cols = out.columns || 80;
  const lines = []; let cur2 = '', w = 0;
  projects.forEach((p, i) => {
    const label = safeText(p);
    const vis = label.length + 2;
    if (w + vis > cols && cur2) { lines.push(cur2); cur2 = ''; w = 0; }
    cur2 += (i === pIdx ? `${INV} ${label} ${R}` : `${D} ${label} ${R}`);
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
  lines.push(`${CY}${safeText(it.name)}${R}  ${kind}`);
  const prompts = it._loaded ? ` · ${it.count} prompt${it.count === 1 ? '' : 's'}` : ''; // count needs the lazy read
  lines.push(`${D}${safeText(it.project)} · ${ago(it.mtime)} ago${prompts} · ${sizeFmt(it.size)}${it.branch ? ' · ' + safeText(it.branch) : ''}${R}`);
  if (it.open) lines.push(`${YE}▸ pick up${R}${D} · ${it.openWhy || 'unfinished'}${R}`);
  if (isArchived(it)) lines.push(`${D}🗄 archived · hidden from other tabs · ^a to unarchive${R}`);
  if (deep) lines.push(`${YE}✓ contains "${safeText(deep.query)}"${R}`);
  const rel = (iso) => iso ? ` [${ago(new Date(iso).getTime())}]` : '';
  const quote = (l) => `${D}│${R} ${l}`; // blockquote gutter marks rendered-markdown blocks apart from plain prompts
  if (it.recap) { // the recap is newer, shorter and says whose move it is — prefer it
    lines.push(`${V}recap${R}${D}${it.recapTs ? ` · ${ts(it.recapTs).trim()}${rel(it.recapTs)}` : ''}${R}`);
    mdLines(it.recap, w - 2).slice(0, 4).forEach((l) => lines.push(quote(l)));
  } else if (it.summary) {
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

function drawHelp() {
  const rows = [
    `${B}sessio${R} ${D}— keys${R}`, '',
    `${CY}← →${R}    switch project tab`,
    `${CY}↑ ↓${R}    move selection ${D}(↓ reveals more)${R}`,
    `${CY}type${R}   fuzzy-filter by name / project / first prompt`,
    ...(RG ? [`${CY}^f${R}     full-text search across all transcripts on disk`] : []),
    `${CY}^w ⌥⌫${R}  delete the last word of the query`,
    `${CY}^u ⌘⌫${R}  clear the whole query`,
    `${CY}^a${R}     archive / unarchive the selected session`,
    `${CY}⇥ ^e${R}   expand / collapse the reply preview`,
    `${CY}↵${R}      resume ${D}(Ghostty: opens a new window, sessio stays open; else in place)${R}`,
    `${CY}^o${R}     resume in ${B}this${R} window ${D}(replaces sessio; use under Ghostty to skip the new window)${R}`,
    `${CY}?${R}      toggle this help`,
    `${CY}esc${R}    clear search, then quit`,
    `${CY}^c${R}     quit`,
    '', `${D}press any key to close${R}`,
  ];
  out.write(CLR + fitFrame(rows, out.columns || 80, out.rows || 24));
}

function draw() {
  if (help) return drawHelp();
  const list = view();
  if (cur >= list.length) cur = Math.max(0, list.length - 1);
  const tabs = tabBar();
  const cols = out.columns || 80;
  const rows = out.rows || 24;
  // The split is decided by the terminal, never by what is in the tab or by what the highlighted
  // session's preview happens to contain. Deriving it from content moved the separator and
  // everything under it on every ←/→, which reads as the UI wandering on its own.
  const MIN_LIST = 3, MIN_PREVIEW = 8;
  const chrome = 1 + tabs.length + 1;                 // header, tab bar, query
  const body = Math.max(0, rows - chrome - 1);        // and the always-present "↓ more" row
  const ceiling = Math.max(MIN_LIST, body - MIN_PREVIEW);
  const want = expand ? MIN_LIST : limit;             // ⇥ hands the room to the reply
  const win = Math.min(Math.max(MIN_LIST, Math.min(want, ceiling)), Math.max(1, body));
  limit = Math.min(limit, ceiling);                   // ↓ may not grow the list past its share
  const previewBox = Math.max(0, body - win);
  const base = preview(list[cur], cols, 0).length;    // preview height without the reply block
  const replyMax = Math.max(1, previewBox - base);
  const prev = preview(list[cur], cols, replyMax);
  if (cur < off) off = cur;
  if (cur >= off + win) off = cur - win + 1;
  if (off < 0) off = 0;
  // The key bar is ~136 columns under Ghostty. It used to wrap on any narrower window, which
  // pushed the frame past `rows` and made every redraw stack (see fitFrame). Drop the least
  // essential hints instead — highest `p` first — until what is left fits on one line. `? help`
  // is p0 because it reveals everything that was dropped.
  const segs = [
    { p: 3, t: `←→ project` },
    { p: 3, t: `↑↓ move` },
    { p: 4, t: `type` },
    ...(RG ? [{ p: 5, t: `^f search-in-text` }] : []),
    { p: 5, t: projects[pIdx] === ARCHIVED_TAB ? `^a unarchive` : `^a archive` },
    { p: 4, t: expand ? `${CY}⇥ collapse${D}` : `⇥ expand-reply` },
    ...(inGhostty()
      ? [{ p: 1, t: `↵ new-window` }, { p: 2, t: `^o same-window` }]
      : [{ p: 1, t: `↵ resume` }]),
    { p: 0, t: `? help` },
    { p: 2, t: `esc quit` },
    { p: 5, t: `${CY}live${D}` },
  ];
  // A flash is why you pressed the key; the hints are always there. Budget the message first and
  // shrink the bar around it, so a message is never clipped down to nonsense.
  const msg = flash ? `  ${G}${safeText(flash)}${R}` : '';
  const room = cols - stripAnsi(msg).length;
  let bar = '';
  for (let cut = 5; cut >= 0; cut--) {
    bar = segs.filter((sg) => sg.p <= cut).map((sg) => sg.t).join(' · ');
    if (stripAnsi(bar).length <= room) break;
  }
  const L = [`${D}${bar}${R}${msg}`];
  L.push(...tabs);
  const prompt = deep ? `${YE}content›${R}` : `${CY}›${R}`;
  L.push(`${prompt} ${safeText(q)}${D}▏${R}${deep ? `  ${YE}${list.length} match${list.length === 1 ? '' : 'es'}${R}` : ''}`);
  const slice = list.slice(off, off + win);
  if (!slice.length) L.push(`${D}  no match${R}`);
  slice.forEach((it, i) => {
    const on = off + i === cur;
    const nm = safeText(it.name).slice(0, 50).padEnd(50);
    const showProj = projects[pIdx] === 'All' || projects[pIdx] === OPEN_TAB;
    const meta = (showProj ? safeText(it.project).slice(0, 16).padEnd(16) + ' ' : '') + sizeFmt(it.size).padStart(5) + ' ' + ago(it.mtime).padStart(3);
    const age = Date.now() - it.mtime; // col0 recency dot: green<5m, orange<24h
    const dot = age < ACTIVE_MS ? `${G}●${R}` : age < RECENT_MS ? `${O}●${R}` : ' ';
    const om = it.open ? `${YE}▸${R}` : ' '; // col1 open marker: pick up where you left off
    if (on) { const bar = `${nm}  ${meta} `.slice(0, cols - 2).padEnd(cols - 2); L.push(`${dot}${om}${INV}${bar}${R}`); }
    else L.push(`${dot}${om}${nm}  ${D}${meta}${R}`);
  });
  for (let i = Math.max(1, slice.length); i < win; i++) L.push(''); // hold the rows open
  const below = list.length - (off + win);
  // The show-more row is always reserved, for the same reason.
  L.push(below > 0 ? `${D} ↓ ${below} more — press ↓ to reveal${R}` : '');
  L.push(...prev);                                                      // then the session details
  out.write(CLR + fitFrame(L, cols, rows));
  flash = ''; // shown for this frame only; the next redraw (keypress or 2s tick) repaints without it
}

// Ghostty has no way to target a sibling pane, but its CLI can open a NEW window running a
// command in the running instance. Returns true if the launch was accepted, false to fall back.
const inGhostty = () => !!(process.env.GHOSTTY_RESOURCES_DIR || process.env.TERM_PROGRAM === 'ghostty');
function ghosttyLaunch(cwd, id) {
  // Run through a login shell: a GUI-launched Ghostty can have a minimal PATH, and `-e claude`
  // would exec directly and fail to find claude/node. Homebrew et al. append to the login
  // profile (~/.zprofile), which `-l` sources. timeout bounds any hang so the TUI can't freeze.
  const shell = process.env.SHELL || '/bin/zsh';
  const args = ['+new-window', `--working-directory=${cwd}`, '-e', shell, '-l', '-c', 'exec claude --resume "$1"', 'sessio', id];
  for (const bin of ['ghostty', '/Applications/Ghostty.app/Contents/MacOS/ghostty']) {
    try { const r = spawnSync(bin, args, { stdio: 'ignore', timeout: 5000 }); if (!r.error && !r.signal) return r.status === 0 || r.status == null; } catch {}
  }
  return false; // ghostty CLI not found / timed out / failed → caller resumes in place
}

readline.emitKeypressEvents(process.stdin);
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.resume();
out.write(HIDE);
draw();
ensureDetail();
out.on('resize', () => draw()); // reflow immediately on terminal resize, not on the next keypress/tick

// live refresh: rescan every 2s, preserving filter, active tab, highlighted row.
let refreshing = false;
const timer = setInterval(async () => {
  if (refreshing) return;
  refreshing = true;
  try {
    const selKey = view()[cur]?.key;
    const activeName = projects[pIdx];
    const activeSearch = deep;
    const refreshed = await load(activeSearch?.files);
    if (deep !== activeSearch) return;
    items = refreshed;
    projects = tabsFor(items);
    const np = projects.indexOf(activeName);
    pIdx = np >= 0 ? np : 0;
    const v = view();
    const ni = selKey ? v.findIndex((i) => i.key === selKey) : -1;
    cur = ni >= 0 ? ni : Math.min(cur, Math.max(0, v.length - 1));
    draw();
    ensureDetail();
  } finally { refreshing = false; }
}, 2000);
timer.unref?.();

// never leave the terminal in raw mode / cursor hidden, and surface the real error
function restore() { try { if (process.stdin.isTTY) process.stdin.setRawMode(false); } catch {} out.write(SHOW); }

// Hand THIS terminal over to `claude --resume`, replacing sessio. Used by ↵ (non-Ghostty,
// or when the Ghostty new-window launch failed) and by ^o (force same-window even under Ghostty).
function resumeInPlace(p) {
  clearInterval(timer);
  if (process.stdin.isTTY) process.stdin.setRawMode(false);
  out.write(CLR + SHOW); // restore cursor before handing the terminal to claude
  const shellQuote = (value) => `'${String(value).replace(/'/g, `'\\''`)}'`;
  const cmd = `cd -- ${shellQuote(p.cwd || '.')} && claude --resume ${shellQuote(p.id)}`;
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
process.on('uncaughtException', (e) => { try { clearInterval(timer); } catch {} restore(); out.write(CLR); console.error('sessions error:', (e && e.stack) || e); process.exit(1); });
process.on('SIGINT', () => { clearInterval(timer); restore(); out.write(CLR); process.exit(0); });

process.stdin.on('keypress', (str, key) => {
  const list = view();
  if (key.ctrl && key.name === 'c') { clearInterval(timer); out.write(CLR); process.exit(0); }
  else if (help) { help = false; draw(); }        // any key closes the help overlay (^c handled above)
  else if (str === '?') { help = true; draw(); }  // open help (before the type-to-filter catch-all)
  else if (key.name === 'escape') {
    if (deep) { deep = null; searchGen++; cur = 0; off = 0; limit = 12; draw(); ensureDetail(); } // first esc clears content search
    else { clearInterval(timer); out.write(CLR); process.exit(0); }  // second esc quits
  }
  else if (key.ctrl && key.name === 'f') {                     // run content search on current query (async)
    if (RG && q) {
      const gen = ++searchGen, term = q;
      out.write(`${CY}searching…${R}\r`);
      contentSearch(term).then(async (res) => {
        if (gen !== searchGen) return; // a newer query/search superseded this one
        if (!res) { flash = 'content search failed'; draw(); return; }
        const matched = await load(res.files);
        if (gen !== searchGen) return;
        deep = { ...res, keys: new Set([...res.files].map((file) => cacheKey(ROOT, file))) };
        items = matched;
        projects = tabsFor(items);
        pIdx = 0;
        cur = 0; off = 0; limit = 12; draw(); ensureDetail();
      });
    }
  }
  else if (key.ctrl && key.name === 'a') {                     // archive/unarchive: sessio-local hide only
    const p = list[cur]; if (!p) return;
    if (isArchived(p)) { archived.delete(archiveKey(p)); archived.delete(p.id); }
    else archived.add(archiveKey(p));
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
  // ⌥⌫ arrives as meta+backspace and ^w does the same job; ⌘⌫ sends ^u in every terminal that
  // binds it (Ghostty ships `super+backspace=text:\x15`) and clears the query outright.
  else if ((key.meta && key.name === 'backspace') || (key.ctrl && key.name === 'w')) { q = q.slice(0, dropWord(q)); requery(); }
  else if (key.ctrl && key.name === 'u') { q = ''; requery(); }
  else if (key.name === 'backspace') { q = q.slice(0, -1); requery(); }
  else if (key.name === 'return') {
    const p = list[cur]; if (!p) return;
    // Ghostty: open the session in a NEW window and keep sessio running as a launcher.
    if (inGhostty() && p.cwd && ghosttyLaunch(p.cwd, p.id)) {
      flash = `↗ opened "${safeText(p.name).slice(0, 40)}" in a new window`;
      draw();
      return; // sessio stays open — pick another session to launch
    }
    resumeInPlace(p); // non-Ghostty, or the new-window launch failed → hand over this window
  }
  else if (key.ctrl && key.name === 'o') { // force resume in THIS window, even under Ghostty
    const p = list[cur]; if (p) resumeInPlace(p);
  }
  else if (str && !key.ctrl && !key.meta && str.length === 1 && str >= ' ') { q += str; deep = null; searchGen++; cur = 0; off = 0; limit = 12; draw(); ensureDetail(); }
});
