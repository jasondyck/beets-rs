"""Diffbeets cases for the tidal plugin that run offline.

The API itself (search, ID/ISRC/barcode lookups, OAuth token handling,
tidalsync) is covered by unit tests against a fake Tidal on 127.0.0.1. Here
both tools have the plugin enabled with a token file that exists (the
config itself), so `import -A` passes the import_begin check without an API
call, and the flexible attribute types and template fields are compared.
"""

ORDER = 115

_IMP = ["import", "-A", "-q", "{music}"]

# A token file that exists is enough for import -A (no lookup is made).
_TIDAL = ("{{plugins: [tidal], tidal: {{tokenfile: '{root}/config.yaml'}}, "
          "import: {{copy: yes, write: no}}}}")

_LS_ITEM = ["ls", "-f",
            "$tidal_track_id|$tidal_track_popularity|$tidal_updated|$tidal_artist_id"]
_LS_ALBUM = ["ls", "-a", "-f",
             "$tidal_album_id|$tidal_album_popularity|$tidal_artist_id"]

ART_CASES = [
    ("tidal-fields", "tracks", {}, _TIDAL, [
        _IMP,
        ["modify", "-y", "-a", "tidal_album_id=251380836",
         "tidal_album_popularity=42", "tidal_artist_id=ar1"],
        ["modify", "-y", "title:One", "tidal_track_id=490839595",
         "tidal_track_popularity=91", "tidal_updated=2024-01-15"],
        _LS_ITEM,
        _LS_ALBUM,
        ["ls", "tidal_track_popularity:90.."],
        ["ls", "-a", "tidal_album_popularity:40..60"],
        # No item has data_source:tidal, so this stays offline.
        ["tidalsync"],
        ["tidalsync", "-a"],
    ]),
]

CASES = [
    # The command's no-token path: zero items, no API call.
    ("tidalsync-empty", ["-p", "tidal", "tidalsync"]),
]
