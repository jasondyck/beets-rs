"""The art phase of diffbeets: fetchart and embedart, offline.

Each case gets its own directory per tool, with the config (`ART_CONFIG`
plus the case's), a library and a fresh copy of generated audio (`tracks`:
two MP3s of one album, or `tracks-art`: the same with an embedded cover)
plus the case's images, made once with ffmpeg (`ART_IMAGES`). The steps
(`beet` commands, with `{music}`, `{src}`, `{url}` and `{root}` filled in)
run in order with both tools; each step's exit status, stdout and stderr are
compared (`art:<case>:<n>`), and then the resulting state
(`art:<case>:state`): the albums' `artpath`/`art_source`, every file under
the library, the music folder and `out/` (images as format and size, via
ImageMagick's `identify`), and each audio file's embedded pictures (count,
and the cover's format and size), read with MediaFile.

Images are compared by format and size only: the two tools' encoders give
different bytes.

Remote art comes from a server in this process on 127.0.0.1, port 0, serving
the generated images under /img/ (and a PNG labelled image/jpeg, an HTML page
and a 404), used through the `cover_art_url` source; no case uses the other
web sources.
"""

import http.server
import os
import re
import shutil
import subprocess
import tempfile
import threading

import yaml

from importphase import merge

ART_CONFIG = """\
directory: {root}/lib
library: {root}/library.db
plugins: [fetchart]
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
fetchart:
    sources: [filesystem]
"""

# name -> ffmpeg lavfi source and size.
ART_IMAGES = {
    "sq300.jpg": ("testsrc", "300x300"),
    "sq300.png": ("testsrc", "300x300"),
    "wide.png": ("testsrc", "400x200"),
    "near.jpg": ("testsrc", "300x295"),
    "small.jpg": ("testsrc", "50x50"),
    "big.jpg": ("testsrc2", "1000x1000"),
    "other.jpg": ("mandelbrot", "300x300"),
    "sq600.jpg": ("testsrc", "600x600"),
}


def build_art_corpus(src, ffmpeg_audio):
    """Audio (`tracks/`, `tracks-art/`) and images (`img/`) under `src`."""
    img = os.path.join(src, "img")
    os.makedirs(img, exist_ok=True)
    for name, (source, size) in ART_IMAGES.items():
        subprocess.run(
            ["ffmpeg", "-loglevel", "error", "-y", "-f", "lavfi", "-i",
             f"{source}=size={size}:rate=1", "-frames:v", "1", "-update", "1",
             os.path.join(img, name)],
            check=True,
        )
    common = {"artist": "Art Artist", "album_artist": "Art Artist", "album": "Art Album",
              "date": "2001"}
    for i, title in enumerate(["One", "Two"], 1):
        tags = dict(common, title=title, track=str(i))
        ffmpeg_audio(os.path.join(src, "tracks", f"{i:02d}.mp3"), "mp3", tags)
        # The same with an embedded front cover.
        out = os.path.join(src, "tracks-art", f"{i:02d}.mp3")
        os.makedirs(os.path.dirname(out), exist_ok=True)
        subprocess.run(
            ["ffmpeg", "-loglevel", "error", "-y",
             "-i", os.path.join(src, "tracks", f"{i:02d}.mp3"),
             "-i", os.path.join(img, "small.jpg"),
             "-map", "0", "-map", "1", "-c", "copy", "-id3v2_version", "3",
             "-metadata:s:v", "comment=Cover (front)", out],
            check=True,
        )
    build_rg_corpus(src)
    build_bpm_corpus(src)
    # convert (`conv/`): a FLAC and an Ogg Vorbis track of one album.
    for i, (title, fmt) in enumerate([("Uno", "flac"), ("Dos", "ogg")], 1):
        tags = dict(common, title=title, track=str(i), album="Conv Album", _d="0.5")
        ffmpeg_audio(os.path.join(src, "conv", f"{i:02d}.{fmt}"), fmt, tags)


# replaygain (`rg/`): an MP3 album over two discs whose tracks differ in
# loudness, an Opus album (R128) and a FLAC single.
RG_AUDIO = [
    ("a/01.mp3", "sine=frequency=440:duration=1", "One", "RG Album", "1"),
    ("a/02.mp3", "anoisesrc=d=1:a=0.3:seed=7", "Two", "RG Album", "1"),
    ("a/03.mp3", "sine=frequency=1000:duration=2,volume=0.3", "Three", "RG Album", "2"),
    ("o/01.opus", "sine=frequency=300:duration=1", "Op One", "RG Opus", "1"),
    ("o/02.opus", "anoisesrc=d=1:a=0.1:seed=3", "Op Two", "RG Opus", "1"),
    ("f/01.flac", "anoisesrc=d=1:a=0.5:seed=5", "Flac One", "RG Flac", "1"),
]


# autobpm (`bpm/`): click tracks (a 40 ms 1 kHz beep every period) whose
# tempos the librosa port estimates.
BPM_AUDIO = [
    ("slow/01.mp3", 60.0 / 100, "Click 100", "BPM Artist", "Slow Clicks", "1"),
    ("fast/01.mp3", 60.0 / 120, "Click 120", "BPM Artist", "Fast Clicks", "1"),
]


def build_bpm_corpus(src):
    for rel, period, title, artist, album, track in BPM_AUDIO:
        out = os.path.join(src, "bpm", rel)
        os.makedirs(os.path.dirname(out), exist_ok=True)
        lavfi = (
            f"aevalsrc='if(lt(mod(t,{period}),0.04),0.9*sin(2*PI*1000*t),0)'"
            ":s=44100:d=8"
        )
        cmd = ["ffmpeg", "-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", lavfi,
               "-ac", "1", "-metadata", f"title={title}", "-metadata", f"artist={artist}",
               "-metadata", f"album={album}", "-metadata", f"track={track}", out]
        subprocess.run(cmd, check=True)


def build_rg_corpus(src):
    for i, (rel, lavfi, title, album, disc) in enumerate(RG_AUDIO, 1):
        out = os.path.join(src, "rg", rel)
        os.makedirs(os.path.dirname(out), exist_ok=True)
        tags = {"title": title, "artist": "RG Artist", "album_artist": "RG Artist",
                "album": album, "track": str(i), "disc": disc}
        cmd = ["ffmpeg", "-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", lavfi]
        for k, v in tags.items():
            cmd += ["-metadata", f"{k}={v}"]
        subprocess.run([*cmd, out], check=True)


class ArtServer:
    """Serves `<src>/img/` on 127.0.0.1 (a free port)."""

    def __init__(self, src):
        img = os.path.join(src, "img")

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):  # noqa: N802
                path = self.path.split("?")[0]
                body, ctype, status = b"", "text/plain", 404
                if path.startswith("/img/"):
                    f = os.path.join(img, os.path.basename(path))
                    if os.path.isfile(f):
                        with open(f, "rb") as fh:
                            body = fh.read()
                        ctype = "image/png" if f.endswith(".png") else "image/jpeg"
                        status = 200
                elif path == "/mislabeled.jpg":
                    with open(os.path.join(img, "sq300.png"), "rb") as fh:
                        body = fh.read()
                    ctype, status = "image/jpeg", 200
                elif path == "/page":
                    body = b"<html><body>" + b"not an image " * 10 + b"</body></html>"
                    ctype, status = "text/html", 200
                self.send_response(status)
                self.send_header("Content-Type", ctype)
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


def describe_image(path):
    """`FORMAT WxH`, as ImageMagick's identify sees it."""
    p = subprocess.run(["magick", "identify", "-format", "%m %wx%h\n", path],
                       capture_output=True, text=True)
    lines = p.stdout.splitlines()
    return lines[0] if p.returncode == 0 and lines else "unreadable image"


def describe_embedded(path):
    import mediafile

    try:
        mf = mediafile.MediaFile(path)
    except Exception as exc:  # noqa: BLE001
        return f"unreadable: {type(exc).__name__}"
    images = mf.images or []
    if not images:
        return "no art"
    with tempfile.NamedTemporaryFile() as f:
        f.write(mf.art)
        f.flush()
        return f"{len(images)} image(s), cover {describe_image(f.name)}"


AUDIO_EXTS = (".mp3", ".flac", ".ogg", ".opus", ".m4a", ".ops")


def describe_converted(path, normalise):
    """A converted file (convert's `out/`): link target, format and tags."""
    import mediafile

    link = ""
    if os.path.islink(path):
        link = f" -> {normalise(os.readlink(path))}"
    try:
        mf = mediafile.MediaFile(path)
    except Exception as exc:  # noqa: BLE001
        return f"unreadable: {type(exc).__name__}{link}"
    images = mf.images or []
    art = f"{len(images)} image(s)"
    if images:
        with tempfile.NamedTemporaryFile() as f:
            f.write(mf.art)
            f.flush()
            art += f", cover {describe_image(f.name)}"
    return (f"{mf.format} {mf.title!r} {mf.artist!r} {mf.album!r} {mf.track} "
            f"{art}{link}")


def snapshot(root, tool, normalise):
    lines = []
    p = subprocess.run([tool.beet, "ls", "-a", "-f", "$album|$artpath|$art_source"],
                       env=dict(tool.env, **{tool.envvar: root}), cwd=root,
                       capture_output=True, text=True, timeout=60)
    lines.append("albums: " + normalise(p.stdout + p.stderr).strip())
    for top in ("lib", "music", "out"):
        base = os.path.join(root, top)
        for dirpath, dirs, files in sorted(os.walk(base)):
            dirs.sort()
            for f in sorted(files):
                full = os.path.join(dirpath, f)
                rel = os.path.relpath(full, root)
                low = f.lower()
                if top == "out" and low.endswith(AUDIO_EXTS):
                    lines.append(f"{rel}: {describe_converted(full, normalise)}")
                elif low.endswith((".m3u", ".m3u8")):
                    # Playlists (convert's, smartplaylist's) byte for byte, as
                    # text with only the case's root replaced (normalise would
                    # drop line endings and the final newline).
                    with open(full, "rb") as fh:
                        body = fh.read().decode("utf-8", "backslashreplace")
                    lines.append(f"{rel}: {body.replace(root, '<ROOT>')!r}")
                elif low.endswith(".mp3"):
                    lines.append(f"{rel}: {describe_embedded(full)}")
                elif low.endswith((".jpg", ".jpeg", ".png", ".webp", ".gif")):
                    lines.append(f"{rel}: {describe_image(full)}")
                else:
                    lines.append(rel)
    return "\n".join(lines)


def run_art_case(work, case, tools, normalise, url):
    """Results [(cid, args, ref outcome, rs outcome)] for one case."""
    name, music, images, extra, steps = case
    outs = {}
    for tool in tools:
        outs[tool.name] = run_one(work, name, music, images, extra, steps, tool,
                                  normalise, url)
    results = []
    for i, step in enumerate(steps):
        args = step[0] if isinstance(step, tuple) else step
        results.append((f"art:{name}:{i + 1}", args, outs["ref"][0][i], outs["rs"][0][i]))
    results.append((f"art:{name}:state", ["(art files, embedded art)"],
                    outs["ref"][1], outs["rs"][1]))
    return results


def run_one(work, name, music, images, extra, steps, tool, normalise, url):
    src = os.path.join(work, "art-src")
    root = os.path.join(work, "art", name, tool.name)
    os.makedirs(root)
    fill = {"root": root, "src": os.path.join(src, "img"), "url": url,
            "music": os.path.join(root, "music")}
    config = merge(yaml.safe_load(ART_CONFIG.format(root=root)),
                   yaml.safe_load(extra.format(**fill)) or {})
    with open(os.path.join(root, "config.yaml"), "w") as f:
        yaml.safe_dump(config, f, allow_unicode=True)
    shutil.copytree(os.path.join(src, music), fill["music"])
    for dest, img in images.items():
        shutil.copy(os.path.join(src, "img", img), os.path.join(fill["music"], dest))
    env = dict(tool.env)
    env[tool.envvar] = root
    env["NO_PROXY"] = "127.0.0.1,localhost"
    env.pop("http_proxy", None)
    env.pop("HTTP_PROXY", None)

    tmp = re.escape(tempfile.gettempdir())

    def norm(text):
        text = text.replace(root, "<ROOT>").replace(src, "<SRC>").replace(url, "<URL>")
        # Python's NamedTemporaryFile names (embedart's similarity check).
        text = re.sub(tmp + r"/tmp[a-z0-9_]{8}", "<TMP>", text)
        return normalise(tool, text, root)

    results = []
    for step in steps:
        # A step: arguments, or (arguments, stdin[, sort_err]); with
        # sort_err, stderr's lines are compared sorted (beets logs some
        # things in set order, e.g. smartplaylist's playlists).
        args, stdin, *opts = step if isinstance(step, tuple) else (step, "")
        argv = [a.format(**fill) for a in args]
        p = subprocess.run([tool.beet, *argv], env=env, cwd=root, input=stdin,
                           capture_output=True, text=True, timeout=120)
        err = norm(p.stderr)
        if "Traceback (most recent call last)" in err:
            err = "CRASH: " + err.strip().splitlines()[-1]
        elif opts and opts[0]:
            err = "\n".join(sorted(err.splitlines())) + ("\n" if err else "")
        results.append({"code": 0 if p.returncode == 0 else 1, "out": norm(p.stdout),
                        "err": err})
    state = {"code": 0, "out": snapshot(root, tool, norm), "err": ""}
    return results, state
