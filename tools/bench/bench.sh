#!/usr/bin/env bash
# Benchmark beets-rs against reference beets on identical synthetic libraries.
#
# usage: tools/bench/bench.sh [n-files] [workdir]
#
#   n-files  corpus size, 10 tracks per album (default 10000)
#   workdir  scratch directory (default: a new mktemp dir); the corpus is
#            kept there and reused on the next run with the same workdir
#
# Needs ffmpeg, a release build (cargo build --release) and reference beets.
# Reference beets is $BEETS_REFERENCE, else ~/.cache/beets-reference/beet.
# Both tools run with a throwaway config/library inside the workdir, so no
# real beets or beets-rs library is touched.
set -euo pipefail

n=${1:-10000}
work=${2:-$(mktemp -d -t beets-bench.XXXXXX)}
here=$(cd "$(dirname "$0")/../.." && pwd)
rs_bin=${BEETS_RS:-$here/target/release/beet}
ref_bin=${BEETS_REFERENCE:-$HOME/.cache/beets-reference/beet}
reps=${REPS:-5}

[[ -x $rs_bin ]] || { echo "no release build at $rs_bin (cargo build --release)" >&2; exit 1; }
[[ -x $ref_bin ]] || { echo "no reference beets at $ref_bin (set BEETS_REFERENCE)" >&2; exit 1; }

# --- corpus: 1 s silent MP3s, 10 per album, 50 artists, 6 genres, 60 years
src=$work/src
mkdir -p "$src"
base=$work/base.mp3
[[ -f $base ]] || ffmpeg -nostdin -loglevel error -y -f lavfi -i anullsrc=r=44100:cl=mono \
    -t 1 -b:a 32k "$base"
genres=(Rock Jazz Pop Electronic Folk Classical)
make_file() {
    local i=$1 a=$(($1 / 10)) t=$(($1 % 10))
    local d; d=$(printf '%s/%05d' "$src" "$a")
    local p; p=$(printf '%s/%02d.mp3' "$d" "$t")
    [[ -f $p ]] && return
    mkdir -p "$d"
    ffmpeg -nostdin -loglevel error -y -i "$base" -c copy -id3v2_version 4 \
        -metadata artist="Artist $((a % 50))" -metadata album_artist="Artist $((a % 50))" \
        -metadata album="Album $a" -metadata title="Song $i ($((t + 1)))" \
        -metadata track="$((t + 1))/10" -metadata date="$((1960 + a % 60))" \
        -metadata genre="${genres[a % 6]}" "$p"
}
export -f make_file
export src base
genres_list=${genres[*]}
for ((i = 0; i < n; i++)); do echo "$i"; done |
    xargs -P "$(nproc)" -I{} bash -c "genres=($genres_list); make_file {}"
echo "corpus: $n files in $((n / 10)) albums ($work)"

for tool in beets beets-rs; do
    root=$work/$tool
    rm -rf "$root"
    mkdir -p "$root"
    cat >"$root/config.yaml" <<EOF
directory: $root/music
library: $root/library.db
plugins: []
import:
  write: no
  copy: no
  autotag: no
  quiet: yes
  log:
paths:
  default: \$albumartist/\$album%aunique{}/\$track \$title
EOF
done

# run <tool> <args...>: prints elapsed seconds
run() {
    local tool=$1; shift
    local bin envvar
    if [[ $tool == beets ]]; then bin=$ref_bin envvar=BEETSDIR; else bin=$rs_bin envvar=BEETSRSDIR; fi
    local t0 t1
    t0=$(date +%s%N)
    env -u BEETSDIR -u BEETSRSDIR "$envvar=$work/$tool" "$bin" "$@" </dev/null >/dev/null 2>"$work/stderr" ||
        echo "  !! $tool $* failed: $(head -c 300 "$work/stderr")" >&2
    t1=$(date +%s%N)
    awk -v a="$t0" -v b="$t1" 'BEGIN {printf "%.4f\n", (b - a) / 1e9}'
}

median() { sort -n | awk '{a[NR] = $1} END {print (NR % 2) ? a[(NR + 1) / 2] : (a[NR / 2] + a[NR / 2 + 1]) / 2}'; }

printf '\n| %-38s | %9s | %9s | %7s |\n' "Command ($n tracks)" beets beets-rs speedup
printf '|%s|%s|%s|%s|\n' "----------------------------------------" "----------:" "----------:" "--------:"
bench() {
    local label=$1 r=$2; shift 2
    local b rs
    b=$(for ((k = 0; k < r; k++)); do run beets "$@"; done | median)
    rs=$(for ((k = 0; k < r; k++)); do run beets-rs "$@"; done | median)
    printf '| %-38s | %8.2fs | %8.3fs | %6.0fx |\n' "$label" "$b" "$rs" "$(awk -v a="$b" -v b="$rs" 'BEGIN {print a / b}')"
}

bench "\`version\` (startup)" "$reps" version
bench "\`import -A\` (no autotag, no copy)" 1 import -A -q "$src"
bench "\`ls\` (all items)" "$reps" ls
bench "\`ls -a\` (all albums)" "$reps" ls -a
bench "\`ls artist:'Artist 7'\`" "$reps" ls "artist:Artist 7"
bench "\`ls title::'^Song 1'\` (regex)" "$reps" ls "title::^Song 1"
bench "\`ls year:1970..1979\`" "$reps" ls year:1970..1979
bench "\`ls -f\` (custom template)" "$reps" ls -f '$albumartist - $album [$year] $track. $title'
bench "\`ls year- title+\` (sorted)" "$reps" ls year- title+
bench "\`stats\`" "$reps" stats
bench "\`mv -p\` (pretend, all items)" 3 mv -p
bench "\`modify -y album:'Album 3' genre=Ska\`" 3 modify -y "album:Album 3" genre=Ska
