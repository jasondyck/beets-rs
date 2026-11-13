"""Diffbeets cases: mbsync."""

ORDER = 60

IMPORT_CASES = []

# ---------------------------------------------------------------------------
# mbsync, in the import phase (against its fake MusicBrainz): a library
# imported from the fixtures, changed, then synced; each step is compared,
# and the ls/db/requests outcomes are taken after the last step.

_MBS = "plugins: [musicbrainz, mbsync]\n"
_REC = "00000000-0000-0000-0001-0000000000"
IMPORT_CASES += [
    ("mbsync-album", "tagged", ["-t"], _MBS, ["a"], [
        ["mbsync", "-p"],
        ["modify", "-y", "-M", "title:Alpha", "title=Wrong", "year=1999", "label=Other"],
        ["modify", "-y", "-M", "title:Beta", "artist=Somebody", "mb_trackid!"],
        ["mbsync", "-p"],
        ["mbsync"],
        ["mbsync"],
        ["ls", "-p"],
    ]),
    ("mbsync-album-nomove", "tagged", ["-t"], _MBS, ["a"], [
        ["modify", "-y", "-M", "-a", "album=Old Name"],
        ["mbsync", "-M", "-f", "$album|$title"],
        ["ls", "-p"],
    ]),
    ("mbsync-album-write", "tagged", ["-t"], _MBS + "import:\n    write: yes\n", ["a"], [
        ["modify", "-y", "-M", "-W", "title:Gamma", "title=Wrong"],
        ["mbsync", "-W", "-M"],
        ["modify", "-y", "-M", "-W", "title:Gamma", "title=Wrong"],
        ["mbsync"],
        ["ls", "-p"],
    ]),
    ("mbsync-skip", "fx", ["-A", "-C"], _MBS, [], [
        ["mbsync"],
        ["mbsync", "-f", "'%if{{$album,$album,$title}}'"],
    ]),
    ("mbsync-notfound", "tagged", ["-t", "-C"], _MBS, ["a"], [
        ["modify", "-y", "-a", "mb_albumid=99999999-9999-9999-9999-999999999999"],
        ["mbsync"],
        ["modify", "-y", "-a", "mb_albumid=not-an-id"],
        ["mbsync"],
    ]),
    ("mbsync-source", "tagged", ["-t", "-C"], _MBS, ["a"], [
        ["modify", "-y", "-M", "title:Alpha", "title=Wrong"],
        ["-P", "musicbrainz", "mbsync"],
        ["modify", "-y", "-a", "data_source=Discogs"],
        ["mbsync"],
    ]),
    ("mbsync-singletons", "fx", ["-A", "-s", "-C"], _MBS, [], [
        ["mbsync"],
        ["modify", "-y", "title:Alpha", f"mb_trackid={_REC}01"],
        ["modify", "-y", "title:Beta", f"mb_trackid={_REC}99"],
        ["mbsync", "-p"],
        ["ls", "-f", "$title|$artist|$mb_trackid|$length"],
        ["mbsync"],
        ["ls", "-f", "$title|$artist|$mb_trackid|$length"],
    ]),
    ("mbsync-va", "sampler", ["-t", "-C"], _MBS, ["a"], [
        ["modify", "-y", "-M", "disc:2", "title=Swapped", "artist=Nobody"],
        ["modify", "-y", "-M", "-a", "albumartist=Someone"],
        ["mbsync", "-p"],
        ["mbsync"],
    ]),
]
IMPORT_CASES += [
    # Files really move: a title and the album renamed (and moved) by
    # modify, then synced back.
    ("mbsync-album-move", "tagged", ["-t"], _MBS, ["a"], [
        ["modify", "-y", "title:Alpha", "title=Wrong"],
        ["modify", "-y", "-a", "album=Old Name"],
        ["ls", "-p"],
        ["mbsync"],
        ["ls", "-p"],
    ]),
    # Info listeners (ftintitle on albuminfo_received) apply to mbsync's
    # lookups too.
    ("mbsync-ftintitle", "sampler", ["-t", "-C"], "plugins: [musicbrainz, mbsync, ftintitle]\n",
     ["a"], [
        ["modify", "-y", "-M", "title:Ocean", "artist=Nobody", "title=Nothing"],
        ["mbsync", "-p"],
        ["mbsync"],
     ]),
]
