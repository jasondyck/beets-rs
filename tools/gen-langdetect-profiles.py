#!/usr/bin/env python3
"""Regenerate src/langdetect/profiles.bin.z from langdetect's profiles.

langdetect 1.0.9 (Apache-2.0) ships its word/n-gram frequency profiles as one
JSON file per language. This script encodes them in a compact binary (see
src/langdetect/profile.rs for the format), keeping langdetect's
`os.listdir()` order: the language list order matters (it is the index order
of the probability vectors and the tie-break order).

Usage:
    python3 tools/gen-langdetect-profiles.py <langdetect>/profiles
"""

import json
import os
import struct
import sys
import zlib

MAGIC = b"LDP1"


def varint(n, out):
    while n >= 0x80:
        out.append((n & 0x7F) | 0x80)
        n >>= 7
    out.append(n)


def main(profile_dir):
    langs = []
    words = {}
    for name in os.listdir(profile_dir):  # langdetect's order
        with open(os.path.join(profile_dir, name), encoding="utf-8") as f:
            data = json.load(f)
        index = len(langs)
        langs.append((data["name"], data["n_words"]))
        for word, freq in data["freq"].items():
            words.setdefault(word, {})[index] = freq

    out = bytearray(MAGIC)
    out.extend(struct.pack("<H", len(langs)))
    for name, n_words in langs:
        raw = name.encode()
        out.append(len(raw))
        out.extend(raw)
        for n in n_words:
            out.extend(struct.pack("<I", n))

    out.extend(struct.pack("<I", len(words)))
    for word in sorted(words):
        raw = word.encode()
        varint(len(raw), out)
        out.extend(raw)
        entries = sorted(words[word].items())
        varint(len(entries), out)
        for index, freq in entries:
            out.append(index)
            varint(freq, out)

    packed = zlib.compress(bytes(out), 9)
    target = os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "src",
        "langdetect",
        "profiles.bin.z",
    )
    with open(target, "wb") as f:
        f.write(packed)
    print(
        f"{len(langs)} languages, {len(words)} n-grams, "
        f"{len(out)} bytes encoded, {len(packed)} bytes compressed -> {target}"
    )


if __name__ == "__main__":
    main(sys.argv[1])
