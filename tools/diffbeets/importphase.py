"""The import phase of diffbeets: both tools import generated audio,
interactively, against a fake MusicBrainz serving tests/fixtures/mb.

The fake server runs in this process, bound to 127.0.0.1 on a free port
(port 0). Each case gets its own directory per tool, holding the config,
the library and a fresh copy of the case's music, so cases can run in
parallel and may change their files mid-import (for "Rescan directory").

A case's answers are fed one per prompt, as each prompt appears (a prompt
is output ending in "? " or ": " followed by a pause); an answer may be an
action on the music directory instead, done when the prompt appears and
before the next answer. When the answers run out, stdin is closed (beets
and beets-rs both abort at end of input).

Each tool's run gets its own fake server, which logs the requests it
gets. Compared per case: the import's exit status, stdout and stderr
(`import:<case>`), `ls` of the items and albums (`import:<case>:ls`), every
stored field of every item and album (`import:<case>:db`, read from the
library database) and the MusicBrainz requests made, with their parameters
(`import:<case>:requests`).
"""

import http.server
import json
import os
import re
import select
import shutil
import sqlite3
import subprocess
import threading
import time
import urllib.parse

import yaml

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
FIXTURES = os.path.join(REPO, "tests", "fixtures", "mb")

# ---------------------------------------------------------------------------
# Fake MusicBrainz


def load_fixtures():
    """Releases by id, and every recording on them by id."""
    releases, recordings = {}, {}
    for name in sorted(os.listdir(FIXTURES)):
        if not (name.startswith("release-") and name.endswith(".json")):
            continue
        with open(os.path.join(FIXTURES, name)) as f:
            rel = json.load(f)
        releases[rel["id"]] = rel
        for medium in rel.get("media", []):
            tracks = list(medium.get("tracks", []))
            tracks += medium.get("data-tracks", [])
            if medium.get("pregap"):
                tracks.append(medium["pregap"])
            for t in tracks:
                rec = dict(t["recording"])
                rec.setdefault("releases", [{"id": rel["id"], "title": rel["title"]}])
                recordings.setdefault(rec["id"], rec)
    return releases, recordings


def query_term(query, field):
    """The value of `field:(...)` in a Lucene query, unescaped, lowercased."""
    m = re.search(rf"{field}:\((.*?)(?<!\\)\)", query, re.I)
    return re.sub(r"\\(.)", r"\1", m.group(1)).lower() if m else ""


class FakeMusicBrainz:
    """Release search returns the releases sharing a word with the query's
    `release:(...)` (every release if it has none); recording search the
    recordings whose title is the query's `recording:(...)`; lookups
    return the fixture. Anything else is a 404. `requests` logs each
    request's path and parameters (without `fmt`)."""

    def __init__(self):
        self.releases, self.recordings = load_fixtures()
        self.requests = []
        fake = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                body = fake.respond(self.path)
                if body is None:
                    self.send_response(404)
                    self.end_headers()
                    return
                data = json.dumps(body).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *args):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.port = self.server.server_address[1]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def respond(self, path):
        url = urllib.parse.urlparse(path)
        parts = [p for p in url.path.split("/") if p]
        if parts[:2] != ["ws", "2"]:
            return None
        parts = parts[2:]
        params = urllib.parse.parse_qs(url.query)
        self.requests.append("/".join(parts) + " " + " ".join(
            f"{k}={v!r}" for k, vs in sorted(params.items()) if k != "fmt" for v in vs))
        query = params.get("query", [""])[0]
        limit = int(params.get("limit", ["25"])[0])
        if parts == ["release"]:
            words = set(query_term(query, "release").split())
            rels = [r for r in self.releases.values()
                    if not words or words & set(r["title"].lower().split())][:limit]
            return {
                "releases": [{"id": r["id"], "title": r["title"], "score": 100} for r in rels],
                "count": len(rels),
                "offset": 0,
            }
        if parts == ["recording"]:
            title = query_term(query, "recording")
            recs = [r for r in self.recordings.values() if r["title"].lower() == title]
            return {"recordings": recs[:limit], "count": len(recs), "offset": 0}
        if len(parts) == 2 and parts[0] == "release":
            return self.releases.get(parts[1])
        if len(parts) == 2 and parts[0] == "recording":
            return self.recordings.get(parts[1])
        return None

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *exc):
        self.server.shutdown()
        self.server.server_close()


# ---------------------------------------------------------------------------
# Driving a tool through its prompts


def looks_like_prompt(text):
    return text.endswith("? ") or text.endswith(": ")


def drive(argv, env, cwd, answers, act, timeout=90):
    """Run `argv`, answering each prompt with the next of `answers`
    (`act(answer)` handles the non-string ones). Returns (code, out, err)."""
    p = subprocess.Popen(argv, env=env, cwd=cwd, stdin=subprocess.PIPE,
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    err_chunks = []
    err_thread = threading.Thread(target=lambda: err_chunks.append(p.stderr.read()))
    err_thread.start()
    out = b""
    since = b""  # output since the last answer
    pending = list(answers)
    deadline = time.monotonic() + timeout
    fd = p.stdout.fileno()
    while True:
        if time.monotonic() > deadline:
            p.kill()
            break
        ready, _, _ = select.select([fd], [], [], 0.2)
        if ready:
            chunk = os.read(fd, 65536)
            if not chunk:
                break
            out += chunk
            since += chunk
            continue
        # A pause: answer if what's shown ends in a prompt.
        if not looks_like_prompt(since.decode("utf-8", "replace")) or p.stdin.closed:
            continue
        while pending and not isinstance(pending[0], str):
            act(pending.pop(0))
        since = b""
        if pending:
            p.stdin.write(pending.pop(0).encode() + b"\n")
            p.stdin.flush()
        else:
            p.stdin.close()
    p.wait()
    err_thread.join()
    if not p.stdin.closed:
        p.stdin.close()
    err = b"".join(err_chunks)
    return p.returncode, out.decode("utf-8", "replace"), err.decode("utf-8", "replace")


# ---------------------------------------------------------------------------
# The phase


def build_import_corpus(src, corpus, ffmpeg):
    for dirname, files in corpus.items():
        for name, tags in files:
            ffmpeg(os.path.join(src, dirname, name), "mp3", tags)


def dump_db(path, normalise):
    """Every stored field of every item and album, one line per field
    (`items <path> field=value`, `albums <album> +flex=value`), sorted;
    ids, mtimes and added times left out, paths normalised."""
    db = sqlite3.connect(path)
    lines = []
    for table, attrs in (("items", "item_attributes"), ("albums", "album_attributes")):
        cols = [c[1] for c in db.execute(f"PRAGMA table_info({table})")]
        rows = {r["id"]: r for r in (dict(zip(cols, row))
                                     for row in db.execute(f"SELECT * FROM {table}"))}
        ident = {eid: normalise(_text(r["path"] if table == "items" else r["album"]))
                 for eid, r in rows.items()}
        for eid, rec in rows.items():
            lines += [f"{table} {ident[eid]}: {c}={normalise(_text(rec[c]))!r}"
                      for c in cols if c not in ("id", "album_id", "mtime", "added")]
        for eid, k, v in db.execute(f"SELECT entity_id, key, value FROM {attrs}"):
            lines.append(f"{table} {ident.get(eid, eid)}: +{k}={normalise(_text(v))!r}")
    db.close()
    return "\n".join(sorted(lines))


def _text(v):
    if isinstance(v, bytes):
        return v.decode("utf-8", "replace")
    return v if isinstance(v, str) else repr(v)


def merge(base, extra):
    """`extra` (a YAML mapping) over `base`, recursively."""
    for k, v in extra.items():
        if isinstance(v, dict) and isinstance(base.get(k), dict):
            merge(base[k], v)
        else:
            base[k] = v
    return base


CASE_CONFIG = """\
directory: {root}/lib
library: {root}/library.db
plugins: [musicbrainz]
import:
    write: no
    resume: no
    incremental: no
    log:
ui:
    color: no
musicbrainz:
    host: 127.0.0.1:{port}
    https: no
    ratelimit: 100
"""


def run_import_case(work, case, tools, normalise):
    """Results [(cid, args, ref outcome, rs outcome)] for one case. A case
    may have a sixth element: `beet` commands run after the import (e.g.
    mbsync), each compared as `import:<case>:<n>`; the ls, db and requests
    outcomes are then taken after them."""
    name, music, args, extra, answers, *rest = case
    after = rest[0] if rest else []
    outcomes = {}
    for tool in tools:
        with FakeMusicBrainz() as mb:
            outcomes[tool.name] = run_one(work, name, music, args, extra, answers,
                                          tool, mb, normalise, after)
    shown = ["import", *args, f"<{music}>"]
    (r, rls, rdb, rreq, rafter) = outcomes["ref"]
    (s, sls, sdb, sreq, safter) = outcomes["rs"]
    return [
        (f"import:{name}", shown, r, s),
        *[(f"import:{name}:{n}", step, ra, sa)
          for n, (step, ra, sa) in enumerate(zip(after, rafter, safter), 1)],
        (f"import:{name}:ls", ["ls", "(items, albums)"], rls, sls),
        (f"import:{name}:db", ["(library.db)"], rdb, sdb),
        (f"import:{name}:requests", ["(MusicBrainz requests)"], rreq, sreq),
    ]


def run_one(work, name, music, args, extra, answers, tool, mb, normalise, after=()):
    """One tool's import for a case: (import, ls, db, requests) outcomes."""
    root = os.path.join(work, "import", name, tool.name)
    os.makedirs(root)
    config = merge(yaml.safe_load(CASE_CONFIG.format(root=root, port=mb.port)),
                   yaml.safe_load(extra) or {})
    with open(os.path.join(root, "config.yaml"), "w") as f:
        yaml.safe_dump(config, f, allow_unicode=True)
    dest = os.path.join(root, "music")
    shutil.copytree(os.path.join(work, "import-src", music), dest)
    env = dict(tool.env)
    env[tool.envvar] = root
    env["PYTHONUNBUFFERED"] = "1"

    def act(action, dest=dest):
        kind, *rest = action
        if kind == "remove":
            os.remove(os.path.join(dest, rest[0]))
        elif kind == "empty":
            for f in os.listdir(dest):
                os.remove(os.path.join(dest, f))
        elif kind == "copy":  # (copy, from import-src path, to name)
            shutil.copy(os.path.join(work, "import-src", rest[0]),
                        os.path.join(dest, rest[1]))

    argv = [tool.beet, "import", *args, dest]
    code, out, err = drive(argv, env, root, answers, act)
    res = {"code": 0 if code == 0 else 1,
           "out": normalise(tool, out, root),
           "err": normalise(tool, err, root)}
    after_res = []
    for step in after:
        p = subprocess.run([tool.beet, *[a.format(root=root) for a in step]], env=env,
                           cwd=root, capture_output=True, text=True, timeout=60,
                           stdin=subprocess.DEVNULL)
        after_res.append({"code": 0 if p.returncode == 0 else 1,
                          "out": normalise(tool, p.stdout, root),
                          "err": normalise(tool, p.stderr, root)})
    ls = []
    for ls_args in (
        ["ls", "-f", "$albumartist|$album|$track|$title|$mb_trackid|$data_source|$path"],
        ["ls", "-a", "-f", "$albumartist|$album|$mb_albumid|$albumtype|$year|$country"],
    ):
        p = subprocess.run([tool.beet, *ls_args], env=env, cwd=root,
                           capture_output=True, text=True, timeout=60)
        ls.append(normalise(tool, p.stdout + p.stderr, root))
    db = dump_db(os.path.join(root, "library.db"),
                 lambda t: normalise(tool, t, root))
    reqs = "\n".join(mb.requests)
    return (res, {"code": 0, "out": "\n".join(ls), "err": ""},
            {"code": 0, "out": db, "err": ""}, {"code": 0, "out": reqs, "err": ""},
            after_res)
