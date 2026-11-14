"""The lyrics phase of diffbeets: the lyrics plugin, offline.

Each case gets its own directory per tool with the config (`LYRICS_CONFIG`
plus the case's), a library and fresh copies of generated audio tracks whose
titles name the lyrics the fake lrcmux backend serves (`LYRICS`). The steps
(`beet import`, `beet lyrics`, `beet ls -a -f ...`) run in order with both
tools and each step's exit status, stdout and stderr are compared.

The point is language detection: `beet lyrics` stores `lyrics_language` (and
`lyrics_translation_language` for fetched translations) from langdetect, and
`lyrics.translate.from_languages` decides whether translating is allowed. No
case reaches Microsoft's translator: the cases that configure `translate`
pick lyrics whose detected language is the target (`already-target`) or is
not in `from_languages` (`from-languages`), so no request is made.
"""

import concurrent.futures
import http.server
import os
import re
import shutil
import subprocess
import tempfile
import threading
import urllib.parse

import yaml

from importphase import merge

LYRICS_CONFIG = """\
directory: {root}/lib
library: {root}/library.db
plugins: [lyrics]
import:
    copy: yes
    write: no
    resume: no
    incremental: no
    log:
ui:
    color: no
paths:
    default: $albumartist/$album/$track $title
lyrics:
    auto: no
    sources: [lrcmux]
    lrcmux:
        url: {url}/lrcmux
"""

# title -> the text the fake lrcmux serves for it (None: 404).
LYRICS = {
    "English": (
        "I woke up this morning with a song in my head\n"
        "The sun was coming through the window and the city was awake\n"
        "There is a quiet kind of music in ordinary things\n"
        "And the rain keeps falling on the old town tonight"
    ),
    "German": (
        "Die Musik ist eine Kunstform, die seit Jahrtausenden die Menschen begleitet\n"
        "Auf dem Marktplatz spielte eine kleine Kapelle und die Kinder tanzten im Kreis\n"
        "Am Abend saßen wir am Fluss und hörten den Wellen zu\n"
        "Es war ein wunderschöner Tag und niemand wollte nach Hause gehen"
    ),
    "Japanese": (
        "音楽は私たちの人生にいつも寄り添っています\n"
        "村の人はみんな古い歌を知っています\n"
        "朝はパンと果物を買いに市場へ行きました\n"
        "太陽はゆっくりと山の向こうに沈んでいきました"
    ),
    "Translated": (
        "Some synced lyrics / Quelques paroles synchronisées\n"
        "Some more synced lyrics / Quelques paroles plus synchronisées\n"
        "The rain keeps falling on the old town tonight / "
        "La pluie continue de tomber sur la vieille ville ce soir"
    ),
    "Instrumental": "[Instrumental]",
    "Featureless": "12345 !!! ??? 67890",
    "Synced": (
        "[00:00.00] I woke up this morning with a song in my head\n"
        "[00:00.50]\n"
        "[00:01.00] The sun was coming through the window and the city was awake"
    ),
    "Missing": None,
}

# (name, extra config, the titles to import)
LYRICS_CASES = [
    ("languages", "{}", ["English", "German", "Japanese", "Translated"]),
    ("instrumental", "{}", ["Instrumental"]),
    ("featureless", "{}", ["Featureless"]),
    ("missing", "{}", ["Missing"]),
    ("synced", "{}", ["Synced"]),
    (
        "already-target",
        "lyrics: {translate: {api_key: secret, to_language: EN, from_languages: [de]}}",
        ["English"],
    ),
    (
        "from-languages",
        "lyrics: {translate: {api_key: secret, to_language: FR, from_languages: [de]}}",
        ["English"],
    ),
]

TRACK_TITLES = list(LYRICS)


def build_lyrics_corpus(src, ffmpeg_audio):
    """One MP3 per titled lyrics text under `src/tracks/`."""
    for i, title in enumerate(TRACK_TITLES, 1):
        tags = {
            "artist": "Lyrics Artist",
            "album_artist": "Lyrics Artist",
            "album": "Lyrics Album",
            "title": title,
            "track": str(i),
        }
        ffmpeg_audio(os.path.join(src, "tracks", f"{i:02d} {title}.mp3"), "mp3", tags)


class LrcMuxServer:
    """Serves `<src>`'s lyrics on 127.0.0.1 (a free port) at `/lrcmux/get`."""

    def __init__(self):
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):  # noqa: N802
                parsed = urllib.parse.urlparse(self.path)
                params = urllib.parse.parse_qs(parsed.query)
                title = (params.get("title") or [""])[0]
                text = LYRICS.get(title) if parsed.path == "/lrcmux/get" else None
                body = (text or "").encode()
                status = 200 if text is not None else 404
                self.send_response(status)
                self.send_header("Content-Type", "text/plain; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *exc):
        self.server.shutdown()
        self.server.server_close()


def run_lyrics_case(work, case, tools, normalise, url):
    """Results [(cid, args, ref outcome, rs outcome)] for one case."""
    name, extra, titles = case
    outs = {
        tool.name: run_one(work, name, extra, titles, tool, normalise, url)
        for tool in tools
    }
    return [
        (f"lyrics:{name}:{i + 1}", args, outs["ref"][i], outs["rs"][i])
        for i, args in enumerate(STEPS)
    ]


STEPS = [
    ["import", "-A", "-q", "{music}"],
    ["lyrics"],
    ["ls", "-f",
     "$lyrics_language|$lyrics_translation_language|$lyrics_backend|$lyrics_instrumental"],
]


def run_one(work, name, extra, titles, tool, normalise, url):
    src = os.path.join(work, "lyrics-src")
    root = os.path.join(work, "lyrics", name, tool.name)
    os.makedirs(root)
    fill = {"root": root, "url": url, "music": os.path.join(root, "music")}
    config = merge(
        yaml.safe_load(LYRICS_CONFIG.format(**fill)),
        yaml.safe_load(extra) or {},
    )
    with open(os.path.join(root, "config.yaml"), "w") as f:
        yaml.safe_dump(config, f, allow_unicode=True)
    os.makedirs(fill["music"])
    for title in titles:
        i = TRACK_TITLES.index(title) + 1
        shutil.copy(
            os.path.join(src, "tracks", f"{i:02d} {title}.mp3"), fill["music"]
        )

    env = dict(tool.env)
    env[tool.envvar] = root
    env["NO_PROXY"] = "127.0.0.1,localhost"
    env.pop("http_proxy", None)
    env.pop("HTTP_PROXY", None)
    tmp = re.escape(tempfile.gettempdir())

    def norm(text):
        text = text.replace(root, "<ROOT>").replace(src, "<SRC>").replace(url, "<URL>")
        text = re.sub(tmp + r"/tmp[a-z0-9_]{8}", "<TMP>", text)
        return normalise(tool, text, root)

    results = []
    for args in STEPS:
        argv = [a.format(**fill) for a in args]
        p = subprocess.run(
            [tool.beet, *argv],
            env=env,
            cwd=root,
            capture_output=True,
            text=True,
            timeout=120,
        )
        err = norm(p.stderr)
        if "Traceback (most recent call last)" in err:
            err = "CRASH: " + err.strip().splitlines()[-1]
        results.append(
            {"code": 0 if p.returncode == 0 else 1, "out": norm(p.stdout), "err": err}
        )
    return results


def run_lyrics_phase(work, ref, rs, opts, ffmpeg, normalise):
    src = os.path.join(work, "lyrics-src")
    build_lyrics_corpus(src, ffmpeg)

    def norm(tool, text, root):
        return normalise(tool, text.replace(root, "<ROOT>"))

    todo = [
        c
        for c in LYRICS_CASES
        if not opts.only
        or any(o in f"lyrics:{c[0]}" or o.startswith("lyrics:" + c[0]) for o in opts.only)
    ]
    with LrcMuxServer() as server:
        with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
            batches = pool.map(
                lambda c: run_lyrics_case(work, c, (ref, rs), norm, server.url), todo
            )
            return [r for batch in batches for r in batch]
