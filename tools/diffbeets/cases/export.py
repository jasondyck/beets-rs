"""Diffbeets cases: export."""

ORDER = 90

ART_CASES = []

# ---------------------------------------------------------------------------
# 5.18c: export, random, playlist and parentwork. Export writes the tags or
# (with -l/-a) the library fields; the playlist query reads the playlist
# smartplaylist wrote; parentwork logs its "No work ..." message (the corpus
# has no MusicBrainz ids, so it makes no requests).
ART_CASES += [
    ("export", "tracks", {}, "{{plugins: [export]}}", [
        ["export", "-f", "json", "-i", "title,album", "artist"],
        ["export", "-f", "jsonlines", "-i", "title,album", "artist"],
        ["export", "-f", "csv", "-i", "title,album", "artist"],
        ["export", "-f", "xml", "-i", "title,album", "artist"],
        ["export", "-l", "-f", "json", "-i", "title,album,path,id", "artist"],
        ["export", "-a", "-f", "json", "-i", "album,albumartist", "Art Artist"],
        ["export", "-f", "json"],
    ]),
    ("random", "tracks", {}, "{{plugins: [random]}}", [
        ["random", "-n", "0"],
        ["random", "-n", "0", "nothing"],
    ]),
    ("parentwork", "tracks", {}, "{{plugins: [parentwork]}}", [
        ["parentwork"],
        ["parentwork", "-f"],
    ]),
    ("playlist", "tracks", {},
     "{{plugins: [playlist, smartplaylist], smartplaylist: {{playlist_dir:"
     " '{root}/out', playlists: [{{name: all.m3u, query: ''}}]}}, playlist:"
     " {{playlist_dir: '{root}/out', relative_to: library}}}}", [
        ["splupdate"],
        ["ls", "playlist:all"],
        ["ls", "-f", "$title", "playlist:{root}/out/all.m3u"],
        ["ls", "playlist:nonexisting"],
    ]),
    # The playlist plugin auto-updates playlists from beets' events (item_moved,
    # cli_exit): `mv -d` moves the files, and the playlist in out/ is rewritten
    # with the new paths (compared byte for byte in the state).
    ("playlist-auto", "tracks", {},
     "{{plugins: [playlist, smartplaylist], smartplaylist: {{playlist_dir:"
     " '{root}/out', playlists: [{{name: all.m3u, query: ''}}]}}, playlist:"
     " {{playlist_dir: '{root}/out', relative_to: library, auto: yes}}}}", [
        ["splupdate"],
        ["mv", "-d", "{root}/moved"],
    ]),
]
