"""Diffbeets cases for the 5.21 plugins that run offline.

freedesktop's command only prints its deprecation message; importsource
writes each imported item's `source_path` (both tools see the same source
paths). The network plugins (acousticbrainz, embyupdate, ipfs, mpdstats,
subsonicplaylist) are covered by unit tests with fake servers/binaries.
"""

ORDER = 110

_IMP = ["import", "-A", "-q", "{music}"]

CASES = [
    ("freedesktop", ["-p", "freedesktop", "freedesktop"]),
]

ART_CASES = [
    (
        "importsource",
        "tracks",
        {},
        "{{plugins: [importsource]}}",
        [_IMP, ["ls", "-f", "$title|$source_path"]],
    ),
]
