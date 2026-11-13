"""Expected values for beets-rs' MusicBrainz parsing and applying tests,
computed by reference beets 2.14 from the releases in tests/fixtures/mb.

For each fixture release and each config in CONFIGS, beets' musicbrainz
plugin parses the release (`album_info`) and the result is written to
tests/fixtures/mb/expected/<release id>.<config>.json:

- `config`: the config (YAML) over the defaults;
- `album`: `dict(album_info)` without `tracks`;
- `tracks`: `dict(track_info)` of each track;
- `album_item_data`: `album_info.item_data` (what the album gets);
- `merged`: `track_info.merge_with_album(album_info)` of each track (what
  each item gets).

Run with the reference venv and a throwaway BEETSDIR:

    BEETSDIR=$(mktemp -d) $HOME/.cache/beets-reference/venv/bin/python \\
        tools/diffbeets/mbexpected.py
"""

import json
import os
import sys

import yaml

HERE = os.path.dirname(os.path.abspath(__file__))
FIXTURES = os.path.join(os.path.dirname(os.path.dirname(HERE)), "tests", "fixtures", "mb")
OUT = os.path.join(FIXTURES, "expected")

CONFIGS = {
    "default": "",
    "options": """
per_disc_numbering: yes
artist_credit: yes
original_date: yes
va_name: VA
musicbrainz:
    genres: yes
    external_ids: {discogs: yes, bandcamp: yes}
overwrite_null:
    album: [asin, albumdisambig]
    track: [arrangers, work, bpm, isrc]
""",
    "aliases": """
import:
    languages: [de]
musicbrainz:
    aliases_as_credits: yes
    genres: yes
    genres_tag: tag
match:
    preferred:
        countries: [DE]
""",
}


def main():
    if not os.environ.get("BEETSDIR"):
        sys.exit("set BEETSDIR to a temporary directory")
    from beets import config
    from beets.util import cached_classproperty

    os.makedirs(OUT, exist_ok=True)
    for name, extra in CONFIGS.items():
        config.clear()
        config.read(user=False, defaults=True)
        # beets caches config-derived class properties (`nullable_fields`)
        # for the process; each config here needs its own.
        cached_classproperty.cache.clear()
        if extra.strip():
            config.set(yaml.safe_load(extra))
        from beetsplug._utils.musicbrainz import MusicBrainzAPI
        from beetsplug.musicbrainz import MusicBrainzPlugin

        plugin = MusicBrainzPlugin()
        for fname in sorted(os.listdir(FIXTURES)):
            if not (fname.startswith("release-") and fname.endswith(".json")):
                continue
            with open(os.path.join(FIXTURES, fname)) as f:
                release = MusicBrainzAPI._normalize_data(json.load(f))
            info = plugin.album_info(release)
            out = {
                "config": extra,
                "album": {k: v for k, v in dict(info).items() if k != "tracks"},
                "tracks": [dict(t) for t in info.tracks],
                "album_item_data": dict(info.item_data),
                "merged": [t.merge_with_album(info) for t in info.tracks],
            }
            path = os.path.join(OUT, f"{release['id']}.{name}.json")
            with open(path, "w") as f:
                json.dump(out, f, indent=1, sort_keys=True, ensure_ascii=False)
                f.write("\n")
            print(path)


if __name__ == "__main__":
    main()
