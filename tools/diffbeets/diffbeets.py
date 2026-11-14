#!/usr/bin/env python3
"""Differential test: beets-rs against reference beets.

Builds the same library twice from a generated corpus of audio files (one
library per tool, same config apart from the library directory), applies the
same `modify` commands to both, then runs a corpus of `ls` queries, `-f`
templates and `mv -p` destinations through both `beet`s and diffs the output.

Only the standard library is used; this script never imports beets-rs code and
only reads the two programs' output. Run it with any Python 3.9+, e.g.

    $HOME/.cache/beets-reference/venv/bin/python tools/diffbeets/diffbeets.py

Options: see --help. Exit status is 1 when there is an unexpected difference
(or an expected one that no longer differs, so the list stays current).

Reference beets always runs through $HOME/.cache/beets-reference/beet with
BEETSDIR=<work>/ref; beets-rs with BEETSRSDIR=<work>/rs. Nothing outside the
work directory is read or written by either tool.
"""

import argparse
import concurrent.futures
import difflib
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import textwrap

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
REF_BEET = os.environ.get("BEETS_REFERENCE") or os.path.expanduser(
    "~/.cache/beets-reference/beet"
)
RS_BEET = os.path.join(REPO, "target", "release", "beet")

sys.dont_write_bytecode = True  # no __pycache__ in the repo
sys.path.insert(0, HERE)
from cases import (  # noqa: E402
    ART_CASES,
    CONFIG,
    ROOT,
    CORPUS,
    EXPECTED,
    EXTRA_CONFIGS,
    IMPORT_CASES,
    IMPORT_CORPUS,
    IMPORTS,
    LAME_CORPUS,
    MODIFIES,
    NORMALIZERS,
    TAG_TEMPLATE,
    TAG_WRITES,
    cases,
)
import importphase  # noqa: E402
import artphase  # noqa: E402
import lyricsphase  # noqa: E402

# ---------------------------------------------------------------------------
# Corpus generation


# ffmpeg output options per corpus format.
FORMATS = {
    "mp3": ["-ar", "44100", "-ac", "2", "-c:a", "libmp3lame", "-b:a", "128k"],
    "mp3vbr": ["-ar", "44100", "-ac", "1", "-c:a", "libmp3lame", "-q:a", "4"],
    "mp3abr": ["-ar", "44100", "-ac", "2", "-c:a", "libmp3lame", "-abr", "1", "-b:a", "96k"],
    "mp3noxing": ["-ar", "22050", "-ac", "1", "-c:a", "libmp3lame", "-b:a", "32k",
                  "-write_xing", "0"],
    "flac": ["-ar", "48000", "-ac", "2", "-c:a", "flac", "-sample_fmt", "s16"],
    "m4a": ["-ar", "44100", "-ac", "2", "-c:a", "aac", "-b:a", "96k"],
    "alac": ["-ar", "44100", "-ac", "2", "-c:a", "alac"],
    "ogg": ["-ar", "44100", "-ac", "2", "-c:a", "libvorbis", "-b:a", "96k"],
    "opus": ["-ac", "2", "-c:a", "libopus", "-b:a", "64k"],
    "wv": ["-ar", "44100", "-ac", "1", "-c:a", "wavpack"],
    "aiff": ["-ar", "48000", "-ac", "2", "-c:a", "pcm_s24be", "-write_id3v2", "1"],
    "wav": ["-ar", "22050", "-ac", "1", "-c:a", "pcm_s16le"],
}


def ffmpeg(path, fmt, tags):
    """Generate a sine file at `path` with `tags` as metadata: 1 second
    long, or `_d` seconds."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tags = dict(tags)
    duration = tags.pop("_d", "1")
    cmd = [
        "ffmpeg", "-nostdin", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", f"sine=frequency=440:duration={duration}",
        "-map_metadata", "-1",
    ] + FORMATS[fmt]
    for key, value in tags.items():
        cmd += ["-metadata", f"{key}={value}"]
    cmd.append(path)
    subprocess.run(cmd, check=True)


def lame(path, opts, tags):
    """An MP3 encoded by the `lame` tool (a real LAME header, which ffmpeg
    doesn't write), with `title`/`artist` tags."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    wav = path + ".wav"
    subprocess.run(["ffmpeg", "-nostdin", "-loglevel", "error", "-y", "-f", "lavfi",
                    "-i", "sine=frequency=440:duration=2.3", "-ac", "2", wav], check=True)
    subprocess.run(["lame", "--quiet", *opts, "--tt", tags["title"], "--ta", tags["artist"],
                    wav, path], check=True)
    os.remove(wav)


def build_corpus(src):
    for rel, fmt, tags in CORPUS:
        ffmpeg(os.path.join(src, rel), fmt, tags)
    if shutil.which("lame"):
        for rel, opts, tags in LAME_CORPUS:
            lame(os.path.join(src, rel), opts, tags)


# ---------------------------------------------------------------------------
# Running both tools


class Tool:
    def __init__(self, name, beet, root, envvar):
        self.name = name
        self.beet = beet
        self.root = root
        self.env = dict(os.environ)
        self.env.pop("BEETSDIR", None)
        self.env.pop("BEETSRSDIR", None)
        self.env[envvar] = root
        self.envvar = envvar
        self.env["COLUMNS"] = "100"
        self.env["LINES"] = "50"
        self.env["TZ"] = "UTC"
        self.env["NO_COLOR"] = "1"
        self.env["PYTHONWARNINGS"] = "ignore"
        self.env["LC_ALL"] = "C.UTF-8"
        # Other directories its output may name, shown as <ROOT> too.
        self.other_roots = []

    def run(self, args, stdin="", cwd=None):
        args = [a.replace(ROOT, self.root) for a in args]
        p = subprocess.run(
            [self.beet, *args],
            env=self.env,
            input=stdin,
            capture_output=True,
            text=True,
            errors="surrogateescape",
            cwd=cwd or self.root,
            timeout=120,
        )
        return p.returncode, p.stdout, p.stderr


NOISE = [
    re.compile(r"^Created database backup at: .*$"),
]


def normalise(tool, text):
    for root in (tool.root, *tool.other_roots):
        text = text.replace(root, "<ROOT>")
    lines = [
        line for line in text.splitlines() if not any(n.match(line) for n in NOISE)
    ]
    return "\n".join(lines)


def outcome(tool, args, cwd=None):
    code, out, err = tool.run(args, cwd=cwd)
    out = normalise(tool, out)
    err = normalise(tool, err)
    # Python tracebacks are reported as a crash, not diffed line by line.
    if "Traceback (most recent call last)" in err:
        err = "CRASH: " + err.strip().splitlines()[-1]
    return {"code": 0 if code == 0 else 1, "out": out, "err": err}


def write_config(root, work):
    os.makedirs(root, exist_ok=True)
    os.makedirs(os.path.join(root, "dest"), exist_ok=True)  # for mv -p -d
    with open(os.path.join(root, "config.yaml"), "w") as f:
        f.write(CONFIG.format(root=root))
    for name, body in EXTRA_CONFIGS.items():
        with open(os.path.join(root, name), "w") as f:
            f.write(body.format(root=root))


def setup(work, ref, rs, verbose):
    src = os.path.join(work, "src")
    print(f"work dir: {work}")
    print("generating corpus...", flush=True)
    build_corpus(src)
    with open(os.path.join(work, "config.yaml"), "w") as f:
        f.write(CONFIG.format(root="<ROOT>"))
    for tool in (ref, rs):
        write_config(tool.root, work)
    steps = [("import", [*args[:-1], os.path.join(src, args[-1])]) for args in IMPORTS]
    steps += [("modify", args) for args in MODIFIES]
    problems = []
    for kind, args in steps:
        results = {}
        for tool in (ref, rs):
            results[tool.name] = outcome(tool, args)
        if verbose:
            print(f"  {kind}: {shlex.join(args)}")
        r, s = results["ref"], results["rs"]
        if r["code"] != 0 or s["code"] != 0:
            problems.append((args, r, s))
    for args, r, s in problems:
        print(f"setup step failed: beet {shlex.join(args)}")
        for name, res in (("ref", r), ("rs", s)):
            print(f"  [{name}] exit {res['code']}\n{textwrap.indent(res['out'] + res['err'], '    ')}")


# ---------------------------------------------------------------------------
# Tag round trips


AUDIO = (".mp3", ".flac", ".m4a", ".ogg", ".opus", ".wv", ".aiff", ".wav")


def mediafile_view(path):
    """The tags of `path` as MediaFile reads them, one `field: value` per
    line. Values beets stores the same way compare equal: a missing string,
    number, flag or list is the same as an empty one (beets writes those
    as empty tags, beets-rs removes them); the nullable floats are kept."""
    import mediafile

    try:
        mf = mediafile.MediaFile(path)
    except Exception as exc:  # noqa: BLE001
        return f"unreadable: {type(exc).__name__}"
    lines = []
    for field in sorted(mediafile.MediaFile.fields()):
        if field in ("art", "images"):
            continue
        value = getattr(mf, field)
        if isinstance(value, list) and not any(value):
            value = None
        if not isinstance(value, float) and not value:
            value = None
        lines.append(f"{field}: {value!r}")
    return "\n".join(lines)


def tag_cases(work, ref, rs):
    """Results (cid, args, ref outcome, rs outcome) of the tag phase."""
    results = []
    for name, args in TAG_WRITES:
        results.append((f"tags:{name}", args, outcome(ref, args, ref.root),
                        outcome(rs, args, rs.root)))
        lib = os.path.join(ref.root, "lib")
        for dirpath, _, files in sorted(os.walk(lib)):
            for f in sorted(files):
                if not f.lower().endswith(AUDIO):
                    continue
                rel = os.path.relpath(os.path.join(dirpath, f), lib)
                ours = os.path.join(rs.root, "lib", rel)
                r = {"code": 0, "out": mediafile_view(os.path.join(lib, rel)), "err": ""}
                if os.path.exists(ours):
                    s = {"code": 0, "out": mediafile_view(ours), "err": ""}
                else:
                    s = {"code": 1, "out": "", "err": "no such file"}
                results.append((f"tags:{name}:{rel}", ["(MediaFile)", rel], r, s))
    # Both read the files reference beets wrote.
    readers = []
    for name, beet, var in (("ref", ref.beet, "BEETSDIR"), ("rs", rs.beet, "BEETSRSDIR")):
        t = Tool(name, beet, os.path.join(work, f"read-{name}"), var)
        write_config(t.root, work)
        outcome(t, ["import", "-A", "-q", "-s", os.path.join(ref.root, "lib")])
        readers.append(t)
    args = ["ls", "-f", TAG_TEMPLATE]
    r, s = (outcome(t, args) for t in readers)
    for res in (r, s):
        res["out"] = "\n".join(sorted(res["out"].splitlines()))
    results.append(("tags:read", ["import", "-s", "<ref-written files>", "&&", *args], r, s))
    return results


# ---------------------------------------------------------------------------
# Interactive imports against a fake MusicBrainz


def import_cases(work, ref, rs, opts):
    """Results of the import phase (see importphase.py)."""
    importphase.build_import_corpus(os.path.join(work, "import-src"), IMPORT_CORPUS, ffmpeg)

    def norm(tool, text, root):
        return normalise(tool, text.replace(root, "<ROOT>"))

    todo = [c for c in IMPORT_CASES
            if not opts.only or any(o in f"import:{c[0]}" or o.startswith("import:" + c[0])
                                    for o in opts.only)]
    with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
        batches = pool.map(
            lambda c: importphase.run_import_case(work, c, (ref, rs), norm), todo)
        return [r for batch in batches for r in batch]


# ---------------------------------------------------------------------------
# fetchart and embedart


def art_cases(work, ref, rs, opts):
    """Results of the art phase (see artphase.py)."""
    src = os.path.join(work, "art-src")
    artphase.build_art_corpus(src, ffmpeg)

    def norm(tool, text, root):
        return normalise(tool, text.replace(root, "<ROOT>"))

    todo = [c for c in ART_CASES
            if not opts.only or any(o in f"art:{c[0]}" or o.startswith("art:" + c[0])
                                    for o in opts.only)]
    with artphase.ArtServer(src) as server:
        with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
            batches = pool.map(
                lambda c: artphase.run_art_case(work, c, (ref, rs), norm, server.url), todo)
            return [r for batch in batches for r in batch]


# ---------------------------------------------------------------------------
# Migration from a beets library


def file_state(path):
    """A file's bytes (hashed), mtime and the names beside it."""
    import hashlib

    with open(path, "rb") as f:
        digest = hashlib.sha256(f.read()).hexdigest()
    return digest, os.stat(path).st_mtime_ns, sorted(os.listdir(os.path.dirname(path)))


def migrate_setup(work, ref, rs):
    """A beets-rs library made by `migrate-beets` from reference beets'
    library (same directory, so the same files), and the results of the
    migration itself: it succeeds and never writes the beets database."""
    mig = Tool("migrated", rs.beet, os.path.join(work, "migrated"), "BEETSRSDIR")
    mig.other_roots = [ref.root]
    write_config(mig.root, work)
    ref_lib = os.path.join(ref.root, "lib")
    path = os.path.join(mig.root, "config.yaml")
    with open(path) as f:
        text = f.read()
    with open(path, "w") as f:
        f.write(text.replace(f"directory: {mig.root}/lib", f"directory: {ref_lib}"))
    db = os.path.join(ref.root, "library.db")
    before = file_state(db)
    args = ["migrate-beets", "--beets-config", os.path.join(ref.root, "config.yaml"), db]
    res = outcome(mig, args)
    after = file_state(db)
    unchanged = {"code": 0, "out": "beets library unchanged", "err": ""}
    state = dict(unchanged, out="beets library unchanged" if before == after
                 else f"beets library changed: {before} -> {after}")
    want = {"code": 0, "out": "Copied <N> items", "err": ""}
    got = dict(res, out=re.sub(r"^Copied \d+ items.*", "Copied <N> items", res["out"]))
    return mig, [
        ("migrate:migrate-beets", args, want, got),
        ("migrate:read-only", args, unchanged, state),
    ]


# ---------------------------------------------------------------------------
# Report


def runs_on_migrated(args):
    """Cases run on the migrated library too: listings, templates, path
    formats, stats and fields (all of them, as none writes)."""
    return any(a in ("ls", "mv", "stats", "fields") for a in args)


def render(res):
    parts = []
    if res["out"]:
        parts.append(res["out"])
    if res["err"]:
        parts.append("[stderr] " + res["err"])
    if res["code"]:
        parts.append(f"[exit {res['code']}]")
    return "\n".join(parts) if parts else "(no output)"


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--workdir", help="parent for the temp dir (default $TMPDIR)")
    ap.add_argument("--keep", action="store_true", help="keep the temp dir")
    ap.add_argument("--only", action="append", default=[],
                    help="run only cases whose id contains this (repeatable)")
    ap.add_argument("--rs", default=RS_BEET, help="beets-rs binary")
    ap.add_argument("--no-build", action="store_true",
                    help="don't run cargo build --release first")
    ap.add_argument("-j", "--jobs", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--show-pass", action="store_true", help="print passing output too")
    ap.add_argument("-v", "--verbose", action="store_true")
    opts = ap.parse_args()

    for tool in ("ffmpeg",):
        if not shutil.which(tool):
            sys.exit(f"{tool} not found")
    if not os.path.exists(REF_BEET):
        sys.exit(f"reference beets wrapper not found: {REF_BEET}")
    if not opts.no_build and opts.rs == RS_BEET:
        subprocess.run(["cargo", "build", "--release", "-q"], cwd=REPO, check=True)

    work = tempfile.mkdtemp(prefix="diffbeets-", dir=opts.workdir)
    try:
        ref = Tool("ref", REF_BEET, os.path.join(work, "ref"), "BEETSDIR")
        rs = Tool("rs", os.path.abspath(opts.rs), os.path.join(work, "rs"), "BEETSRSDIR")
        setup(work, ref, rs, opts.verbose)

        def wanted(cid):
            return not opts.only or any(o in cid for o in opts.only)

        # A migrated case needs its plain case's reference output.
        todo = [c for c in cases() if wanted(c[0]) or wanted(f"migrate:{c[0]}")]
        seen = set()
        for cid, _ in todo:
            if cid in seen:
                sys.exit(f"duplicate case id: {cid}")
            seen.add(cid)
        print(f"running {len(todo)} cases...", flush=True)

        def run_case(case):
            cid, args = case
            # From inside the library directory: implicit relative path
            # queries need the path to exist there (beets checks the cwd, then
            # matches relative to the library directory).
            r = outcome(ref, args, os.path.join(ref.root, "lib"))
            s = outcome(rs, args, os.path.join(rs.root, "lib"))
            if normalise_case := NORMALIZERS.get(cid):
                r = dict(r, out=normalise_case(r["out"]), err=normalise_case(r["err"]))
                s = dict(s, out=normalise_case(s["out"]), err=normalise_case(s["err"]))
            return cid, args, r, s

        with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
            results = list(pool.map(run_case, todo))
        if any(wanted(f"migrate:{cid}") for cid, _ in todo) or wanted("migrate:"):
            # The same cases on a library migrated from reference beets'
            # (before the tag phase rewrites its files), against beets'
            # output on its own library.
            print("migrated library...", flush=True)
            mig, mig_results = migrate_setup(work, ref, rs)
            ref_out = {cid: r for cid, _, r, _ in results}

            def run_migrated(case):
                cid, args = case
                return (f"migrate:{cid}", args, ref_out[cid],
                        outcome(mig, args, os.path.join(ref.root, "lib")))

            todo_mig = [c for c in todo if wanted(f"migrate:{c[0]}") and runs_on_migrated(c[1])]
            with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
                mig_results += list(pool.map(run_migrated, todo_mig))
            mig_results = [r for r in mig_results if wanted(r[0])]
            seen.update(r[0] for r in mig_results)
            results = [r for r in results if wanted(r[0])] + mig_results
        if not opts.only or any(o.startswith("tags") or o in "tags:" for o in opts.only):
            print("tag round trips...", flush=True)
            tag_results = [
                r for r in tag_cases(work, ref, rs)
                if not opts.only or any(o in r[0] for o in opts.only)
            ]
            seen.update(r[0] for r in tag_results)
            results += tag_results

        if not opts.only or any(o.startswith("import") or o in "import:" for o in opts.only):
            print("import phase...", flush=True)
            import_results = import_cases(work, ref, rs, opts)
            import_results = [
                r for r in import_results
                if not opts.only or any(o in r[0] for o in opts.only)
            ]
            seen.update(r[0] for r in import_results)
            results += import_results

        if not opts.only or any(o.startswith("art") or o in "art:" for o in opts.only):
            print("art phase...", flush=True)
            art_results = [
                r for r in art_cases(work, ref, rs, opts)
                if not opts.only or any(o in r[0] for o in opts.only)
            ]
            seen.update(r[0] for r in art_results)
            results += art_results

        if not opts.only or any(o.startswith("lyrics") or o in "lyrics:" for o in opts.only):
            print("lyrics phase...", flush=True)
            lyrics_results = [
                r for r in lyricsphase.run_lyrics_phase(work, ref, rs, opts, ffmpeg, normalise)
                if not opts.only or any(o in r[0] for o in opts.only)
            ]
            seen.update(r[0] for r in lyrics_results)
            results += lyrics_results

        counts = {"PASS": 0, "DIFF": 0, "EXPECTED": 0, "FIXED?": 0}
        for cid, args, r, s in results:
            same = r == s
            expected = EXPECTED.get(cid)
            if expected is None and cid.startswith("migrate:"):
                expected = EXPECTED.get(cid.removeprefix("migrate:"))
            if same and not expected:
                status = "PASS"
            elif same:
                status = "FIXED?"
            elif expected:
                status = "EXPECTED"
            else:
                status = "DIFF"
            counts[status] += 1
            if status == "PASS" and not opts.show_pass:
                continue
            print(f"\n=== {status}: {cid}")
            print(f"    beet {shlex.join(args)}")
            if expected:
                print(f"    expected difference: {expected}")
            if same:
                print(textwrap.indent(render(r), "    "))
                continue
            a, b = render(r).splitlines(), render(s).splitlines()
            if len(a) + len(b) <= 30:
                print("  --- beets")
                print(textwrap.indent(render(r), "    "))
                print("  --- beets-rs")
                print(textwrap.indent(render(s), "    "))
            else:
                for line in difflib.unified_diff(a, b, "beets", "beets-rs", lineterm="", n=2):
                    print("    " + line)
        stale = [k for k in EXPECTED if k not in seen and not opts.only]
        for k in stale:
            print(f"\n=== STALE expected-difference entry (no such case): {k}")
        print(
            f"\n{len(results)} cases: {counts['PASS']} pass, {counts['DIFF']} differ, "
            f"{counts['EXPECTED']} expected differences, "
            f"{counts['FIXED?']} expected but now equal"
        )
        return 1 if counts["DIFF"] or counts["FIXED?"] or stale else 0
    finally:
        if opts.keep:
            print(f"kept {work}")
        else:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
