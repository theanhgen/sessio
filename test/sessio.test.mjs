import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { sanitizeTerminalText } from '../lib/safety.mjs';
import { cacheKey, pruneCache, selectTranscriptRows } from '../lib/session-store.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const cli = path.join(root, 'bin', 'sessio.mjs');
const source = fs.readFileSync(cli, 'utf8');

test('removes control sequences from transcript text before terminal rendering', () => {
  const raw = 'safe\x1b]52;c;secret\x07 text\r\nnext\u0000';

  assert.equal(sanitizeTerminalText(raw), 'safe]52;c;secret text\nnext');
});

test('includes content-search matches beyond the browsing cap', () => {
  const rows = Array.from({ length: 301 }, (_, index) => ({
    file: `/transcripts/${index}.jsonl`,
    mtime: 301 - index,
  }));
  const matched = rows[300];

  const selected = selectTranscriptRows(rows, 300, new Set([matched.file]));

  assert.equal(selected.length, 301);
  assert.ok(selected.some((row) => row.file === matched.file));
});

test('uses a path-based cache key and prunes absent sessions', () => {
  const rootDir = '/Users/example/.claude/projects';
  const keep = '/Users/example/.claude/projects/-Users-example-work/keep.jsonl';
  const drop = '/Users/example/.claude/projects/-Users-example-work/drop.jsonl';
  const key = cacheKey(rootDir, keep);
  const cache = new Map([[key, { first: 'keep' }], [cacheKey(rootDir, drop), { first: 'drop' }]]);

  const pruned = pruneCache(cache, new Set([key]));

  assert.deepEqual([...pruned.keys()], [key]);
  assert.equal(key, '-Users-example-work/keep.jsonl');
});

test('exits successfully with a helpful empty-state message when no transcript directory exists', () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'sessio-empty-home-'));
  try {
    const output = execFileSync(process.execPath, [cli], {
      encoding: 'utf8',
      env: { ...process.env, HOME: home, SESSIO_NO_UPDATE: '1' },
    });

    assert.match(output, /No sessions found\./);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test('updates are opt-in and never git-pull the running checkout', () => {
  assert.match(source, /includes\('--update'\)/);
  assert.doesNotMatch(source, /spawnSync\('git'/);
});

test('README and website describe the same Ghostty and archive controls', () => {
  const readme = fs.readFileSync(path.join(root, 'README.md'), 'utf8');
  const website = fs.readFileSync(path.join(root, 'docs', 'index.html'), 'utf8');

  for (const document of [readme, website]) {
    assert.match(document, /\^a/);
    assert.match(document, /\^o/);
    assert.match(document, /Ghostty/);
  }
});
