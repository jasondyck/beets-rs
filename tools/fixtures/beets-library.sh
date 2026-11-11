#!/bin/sh
# Regenerate tests/fixtures/beets-2.14-library.sql: a small synthetic beets
# library (generated audio, imported with -A by the reference beets at
# ~/.cache/beets-reference, in a temp BEETSDIR), dumped as SQL. Usage:
#   tools/fixtures/beets-library.sh WORKDIR
# WORKDIR must be a fresh, empty directory.
set -eu
W=$(cd "$1" && pwd)
OUT="$(cd "$(dirname "$0")/../.." && pwd)/tests/fixtures/beets-2.14-library.sql"
B="$HOME/.cache/beets-reference/beet"
PY="$HOME/.cache/beets-reference/venv/bin/python"
mkdir -p "$W/src/album" "$W/beets"
gen() { ffmpeg -loglevel error -f lavfi -i sine=d=1 "$@"; }
gen -metadata artist=Artist -metadata album_artist=Artist -metadata album='Ünïcode Album' \
    -metadata title=One -metadata track=1 -metadata genre='Rock; Pop' -metadata date=2001 "$W/src/album/1.mp3"
gen -metadata artist=Artist -metadata album_artist=Artist -metadata album='Ünïcode Album' \
    -metadata title=Two -metadata track=2 -metadata date=2001 "$W/src/album/2.mp3"
gen -metadata artist=Solo -metadata title='Single ½' "$W/src/single.mp3"
printf 'directory: %s/beets/music\nlibrary: %s/beets/library.db\n' "$W" "$W" > "$W/beets/config.yaml"
export BEETSDIR="$W/beets"
cd "$W"
"$B" import -A -q src/album >/dev/null
"$B" import -A -q -s src/single.mp3 >/dev/null
"$B" modify -y title:One mood=happy >/dev/null
"$B" modify -a -y album:Ünïcode rating=5 >/dev/null
"$B" modify -y title:Single comments=hello >/dev/null
"$PY" -c '
import sqlite3, sys
c = sqlite3.connect(sys.argv[1])
# Timestamps vary run to run; pin them.
c.execute("UPDATE items SET added = 1700000000.5, mtime = 1700000001.0")
c.execute("UPDATE albums SET added = 1700000000.25")
c.commit()
with open(sys.argv[2], "w") as f:
    for line in c.iterdump():
        f.write(line + "\n")
' "$W/beets/library.db" "$OUT"
echo "wrote $OUT"
