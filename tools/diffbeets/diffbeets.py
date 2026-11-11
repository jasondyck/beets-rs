#!/usr/bin/env python3
"""Differential test: beets-rs against reference beets.

Builds the same library twice from a generated corpus of audio files (one
library per tool, same config apart from the library directory), applies the
same `modify` commands to both, then runs a corpus of `ls` queries, `-f`
templates and `mv -p` destinations through both `beet`s and diffs the output.

Only the standard library is used; this script never imports beets-rs code and
only reads the two programs' output. Run it with any Python 3.9+, e.g.

    ~/.cache/beets-reference/venv/bin/python tools/diffbeets/diffbeets.py

Options: see --help. Exit status is 1 when there is an unexpected difference
(or an expected one that no longer differs, so the list stays current).

Reference beets always runs through ~/.cache/beets-reference/beet with
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
REF_BEET = os.path.expanduser("~/.cache/beets-reference/beet")
RS_BEET = os.path.join(REPO, "target", "release", "beet")

sys.dont_write_bytecode = True  # no __pycache__ in the repo
sys.path.insert(0, HERE)
from cases import (  # noqa: E402
    CONFIG,
    ROOT,
    CORPUS,
    EXPECTED,
    EXTRA_CONFIGS,
    IMPORTS,
    MODIFIES,
    cases,
)

# ---------------------------------------------------------------------------
# Corpus generation


def ffmpeg(path, fmt, tags):
    """Generate a 1-second sine file at `path` with `tags` as metadata."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    cmd = [
        "ffmpeg", "-nostdin", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "sine=frequency=440:duration=1",
        "-map_metadata", "-1",
    ]
    if fmt == "mp3":
        cmd += ["-ar", "44100", "-ac", "2", "-c:a", "libmp3lame", "-b:a", "128k"]
    elif fmt == "flac":
        cmd += ["-ar", "48000", "-ac", "2", "-c:a", "flac", "-sample_fmt", "s16"]
    elif fmt == "m4a":
        cmd += ["-ar", "44100", "-ac", "2", "-c:a", "aac", "-b:a", "96k"]
    elif fmt == "ogg":
        cmd += ["-ar", "44100", "-ac", "2", "-c:a", "libvorbis", "-b:a", "96k"]
    else:
        raise ValueError(fmt)
    for key, value in tags.items():
        cmd += ["-metadata", f"{key}={value}"]
    cmd.append(path)
    subprocess.run(cmd, check=True)


def build_corpus(src):
    for rel, fmt, tags in CORPUS:
        ffmpeg(os.path.join(src, rel), fmt, tags)


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
        self.env["COLUMNS"] = "100"
        self.env["LINES"] = "50"
        self.env["TZ"] = "UTC"
        self.env["NO_COLOR"] = "1"
        self.env["PYTHONWARNINGS"] = "ignore"
        self.env["LC_ALL"] = "C.UTF-8"

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
    text = text.replace(tool.root, "<ROOT>")
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
# Report


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

        todo = [c for c in cases() if not opts.only or any(o in c[0] for o in opts.only)]
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
            return (
                cid,
                args,
                outcome(ref, args, os.path.join(ref.root, "lib")),
                outcome(rs, args, os.path.join(rs.root, "lib")),
            )

        with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
            results = list(pool.map(run_case, todo))

        counts = {"PASS": 0, "DIFF": 0, "EXPECTED": 0, "FIXED?": 0}
        for cid, args, r, s in results:
            same = r == s
            expected = EXPECTED.get(cid)
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
