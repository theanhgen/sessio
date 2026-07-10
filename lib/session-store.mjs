import path from 'node:path';

export function cacheKey(root, file) {
  return path.relative(root, file);
}

export function selectTranscriptRows(rows, cap, extraFiles = new Set()) {
  const selected = new Map(rows.slice(0, cap).map((row) => [row.file, row]));
  for (const row of rows) {
    if (extraFiles.has(row.file)) selected.set(row.file, row);
  }
  return [...selected.values()].sort((a, b) => b.mtime - a.mtime);
}

export function pruneCache(cache, keys) {
  return new Map([...cache].filter(([key]) => keys.has(key)));
}
