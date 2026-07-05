#!/bin/bash
# Back up Claude Code session transcripts to iCloud Drive (macOS).
# Incremental (rsync), no --delete: a session removed locally stays in the backup,
# so an accidental local wipe or `claude` cleanup can't erase your history.
#
# Override SRC/DEST via environment if your layout differs.
set -euo pipefail

SRC="${SESSIO_BACKUP_SRC:-$HOME/.claude/projects/}"
DEST="${SESSIO_BACKUP_DEST:-$HOME/Library/Mobile Documents/com~apple~CloudDocs/claude-sessions-backup/}"

mkdir -p "$DEST"
rsync -a --exclude '.DS_Store' "$SRC" "$DEST"
echo "$(date '+%Y-%m-%d %H:%M')  backed up $(du -sh "$SRC" | cut -f1) -> $DEST" >> "$HOME/.claude/backup-sessions.log"
