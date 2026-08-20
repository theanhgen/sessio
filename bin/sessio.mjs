#!/usr/bin/env node
// Launcher for the sessio native binary.
//
// The package itself carries no implementation: the per-platform binary arrives as an optional
// dependency, so `npm install` downloads exactly one. This file finds it and hands the process
// over with execve, so sessio *is* the process — no idle node parent holding the terminal, and
// signals reach the TUI directly.
//
// `--update` stays here rather than in the binary: only the launcher knows how sessio was
// installed, and a self-updating binary can't tell a global npm install from a cargo build.

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import https from 'node:https';
import path from 'node:path';
import process from 'node:process';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const PKG_ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), '..');

/** musl reports no glibc runtime version, which is how we tell the two linux builds apart. */
function isMusl() {
  try {
    const report = typeof process.report?.getReport === 'function' ? process.report.getReport() : null;
    if (report?.header && !report.header.glibcVersionRuntime) return true;
  } catch {}
  return false;
}

function platformPackage() {
  const { platform, arch } = process;
  if (platform === 'darwin' && arch === 'arm64') return 'sessio-darwin-arm64';
  if (platform === 'darwin' && arch === 'x64') return 'sessio-darwin-x64';
  if (platform === 'linux' && arch === 'x64') return isMusl() ? 'sessio-linux-x64-musl' : 'sessio-linux-x64';
  // There is no aarch64-musl build. sessio-linux-arm64 is glibc-only and declares it, so
  // handing it to a musl host would either be skipped by npm or fail on a missing loader —
  // say so plainly instead of failing later with a confusing message.
  if (platform === 'linux' && arch === 'arm64') return isMusl() ? null : 'sessio-linux-arm64';
  return null;
}

function findBinary() {
  const pkg = platformPackage();
  if (!pkg) {
    const musl = process.platform === 'linux' && isMusl() ? ' (musl)' : '';
    return {
      error:
        `sessio has no prebuilt binary for ${process.platform}-${process.arch}${musl}.\n` +
        'Build from source instead:\n' +
        '  cargo install --git https://github.com/theanhgen/sessio',
    };
  }
  try {
    // Resolve the package's own manifest — the binary sits beside it.
    const manifest = require.resolve(`${pkg}/package.json`);
    const bin = path.join(path.dirname(manifest), 'bin', 'sessio');
    if (fs.existsSync(bin)) return { bin };
    return { error: `${pkg} is installed but its binary is missing (looked in ${bin}).` };
  } catch {
    return {
      error: `${pkg} is not installed.\nIf you used --no-optional, reinstall without it:\n  npm i -g sessio`,
    };
  }
}

// --- explicit update (sessions --update) -------------------------------------------------
// Starting a session browser must never modify its checkout or global installation. Updates are
// deliberately opt-in and never run `git pull` in a developer checkout.

const readPkg = (root) => {
  try {
    return JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
  } catch {
    return null;
  }
};
/** Split a version into its numeric core and prerelease identifiers, dropping build metadata. */
export function parseSemver(v) {
  const s = String(v).split('+')[0];
  const dash = s.indexOf('-'); // first only: `1.0.0-alpha-beta.1` has one prerelease part
  const core = dash === -1 ? s : s.slice(0, dash);
  const pre = dash === -1 ? null : s.slice(dash + 1);
  return {
    nums: core.split('.').map((n) => (/^\d+$/.test(n) ? Number(n) : 0)),
    pre: pre ? pre.split('.') : null,
  };
}

/** SemVer §11 precedence for prerelease identifiers. */
function comparePre(a, b) {
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const x = a[i], y = b[i];
    if (x === undefined) return -1; // a smaller set of fields has lower precedence
    if (y === undefined) return 1;
    const nx = /^\d+$/.test(x), ny = /^\d+$/.test(y);
    if (nx && ny) {
      if (Number(x) !== Number(y)) return Number(x) < Number(y) ? -1 : 1;
    } else if (nx !== ny) {
      return nx ? -1 : 1; // numeric identifiers rank below alphanumeric ones
    } else if (x !== y) {
      return x < y ? -1 : 1;
    }
  }
  return 0;
}

/**
 * True when `a` is strictly newer than `b`.
 *
 * Prereleases have to be handled properly: splitting on '.' and calling Number() turns
 * `1.0.0-alpha.1` into [1, 0, NaN, 1], so the patch collapsed to 0 and anyone on an alpha was
 * told the stable release "is current" instead of being offered the upgrade.
 */
export function isNewer(a, b) {
  const pa = parseSemver(a), pb = parseSemver(b);
  for (let i = 0; i < 3; i++) {
    const x = pa.nums[i] || 0, y = pb.nums[i] || 0;
    if (x !== y) return x > y;
  }
  if (!pa.pre && !pb.pre) return false;
  if (!pa.pre) return true;  // a release outranks any prerelease of the same core
  if (!pb.pre) return false;
  return comparePre(pa.pre, pb.pre) > 0;
}
const writable = (p) => {
  try { fs.accessSync(p, fs.constants.W_OK); return true; } catch { return false; }
};

function latestVersion(name, timeoutMs) {
  return new Promise((resolve) => {
    // Full packument (abbreviated form) — the /<name>/latest endpoint returns empty on npm, so
    // read dist-tags.latest from the package doc instead.
    const req = https.get(
      `https://registry.npmjs.org/${name}`,
      { timeout: timeoutMs, headers: { accept: 'application/vnd.npm.install-v1+json' } },
      (res) => {
        if (res.statusCode !== 200) { res.resume(); return resolve(null); }
        res.on('error', () => resolve(null)); // a mid-transfer drop would otherwise throw
        let d = '';
        res.on('data', (c) => { d += c; });
        res.on('end', () => {
          try { resolve(JSON.parse(d)['dist-tags']?.latest ?? null); } catch { resolve(null); }
        });
      },
    );
    req.on('timeout', () => { req.destroy(); resolve(null); });
    req.on('error', () => resolve(null));
  });
}

async function update() {
  if (process.env.NO_UPDATE_NOTIFIER || process.env.SESSIO_NO_UPDATE) {
    console.log('Updates are disabled by NO_UPDATE_NOTIFIER or SESSIO_NO_UPDATE.');
    return;
  }
  const pkg = readPkg(PKG_ROOT);
  if (!pkg?.name || !pkg?.version) return;
  const cur = pkg.version;
  const latest = await latestVersion(pkg.name, 2000); // bounded: offline never delays launch
  if (!latest) { console.error('Could not check npm for a newer sessio version.'); process.exitCode = 1; return; }
  if (!isNewer(latest, cur)) { console.log(`sessio ${cur} is current.`); return; }

  if (fs.existsSync(path.join(PKG_ROOT, '.git'))) {
    process.stdout.write(`sessio ${latest} available — update this checkout yourself:\n  git -C ${PKG_ROOT} pull\n`);
    return;
  }
  const gRoot = (() => {
    try { return spawnSync('npm', ['root', '-g'], { encoding: 'utf8', timeout: 5000 }).stdout.trim(); } catch { return ''; }
  })();
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

// --- entry -------------------------------------------------------------------------------

// Only launch when run directly. The version helpers above are exported for unit tests, and
// importing this file must not start a TUI or exit the test runner.
// Both sides have to be resolved through symlinks: npm links the command as
// node_modules/.bin/sessions -> ../sessio/bin/sessio.mjs, and Node reports that symlink in
// argv[1] while import.meta.url is already the real file. Comparing them unresolved made this
// false for every npm install, so `sessions` exited 0 having done nothing at all.
const isMain = (() => {
  const invoked = process.argv[1];
  if (!invoked) return false;
  const self = fileURLToPath(import.meta.url);
  try {
    return fs.realpathSync(invoked) === fs.realpathSync(self);
  } catch {
    return path.resolve(invoked) === self;
  }
})();

if (isMain) {
  const argv = process.argv.slice(2);

  if (argv.includes('--update')) {
    try { await update(); } catch { console.error('Could not update sessio.'); process.exitCode = 1; }
    process.exit(process.exitCode || 0);
  }

  const { bin, error } = findBinary();
  if (error) {
    console.error(error);
    process.exit(1);
  }

  // stdio: 'inherit' keeps the TUI attached to the real terminal. Node has no execve, so the
  // launcher stays alive as a thin parent and forwards the child's exit status and signals.
  const r = spawnSync(bin, argv, { stdio: 'inherit' });
  if (r.error) {
    console.error(`Could not start sessio: ${r.error.message}`);
    process.exit(1);
  }
  if (r.signal) {
    // Re-raise so the shell sees the real cause of death (e.g. ^C -> SIGINT) rather than exit 0.
    process.kill(process.pid, r.signal);
  }
  process.exit(r.status ?? 0);
}
