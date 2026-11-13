"""Diffbeets cases: the rarely used 5.20 plugins that run offline:
mbsubmit, play (with `echo` as the player), keyfinder (its program is
missing: both tools log the same error) and autobpm (the librosa port on
click tracks; the library importer is compared too)."""

import os

ORDER = 95

ART_CASES = []

# `play` never launches a real player here: `play.command: echo` with
# `raw: yes` hands it the files and its output is comparable. The warning
# threshold prompt (and its "Continue"/"Abort" answers) is exercised too,
# as is `relative_to`, which makes the paths relative to the library.
ART_CASES += [
    ("play", "tracks", {},
     "{{plugins: [play], play: {{command: echo, raw: yes, relative_to:"
     " '{root}/music', warning_threshold: 0}}}}", [
        ["import", "-A", "-q", "{music}"],
        ["play", "-y"],
        ["play", "-y", "-a"],
        ["play", "-y", "-A", "extra"],
        ["play", "-y", "nothing"],
    ]),
    ("play-threshold", "tracks", {},
     "{{plugins: [play], play: {{command: echo, raw: yes, warning_threshold: 1}}}}", [
        ["import", "-A", "-q", "{music}"],
        (["play"], "a"),
        (["play"], "c"),
        ["play", "-y"],
    ]),
    ("mbsubmit", "tracks", {}, "{{plugins: [mbsubmit]}}", [
        ["import", "-A", "-q", "{music}"],
        ["mbsubmit"],
        ["mbsubmit", "-l", "2"],
        ["mbsubmit", "title:One"],
    ]),
    ("mbsubmit-format", "tracks", {},
     "{{plugins: [mbsubmit], mbsubmit: {{format: '$disc-$track $title',"
     " threshold: none}}}}", [
        ["import", "-A", "-q", "{music}"],
        ["mbsubmit"],
    ]),
    ("keyfinder", "tracks", {}, "{{plugins: [keyfinder]}}", [
        ["import", "-A", "-q", "{music}"],
        ["keyfinder"],
        ["ls", "-f", "$title|$initial_key"],
    ]),
    # autobpm: the import computes the tempos (`auto: yes`), then the
    # command's "already exists", quiet and force paths; a silent-ish
    # corpus would give 0 BPM in both tools, the click tracks give 99 and
    # 117 (librosa's own estimates).
    ("autobpm", "bpm", {}, "{{plugins: [autobpm]}}", [
        ["import", "-A", "-q", "{music}"],
        ["ls", "-f", "$title|$bpm"],
        ["autobpm"],
        ["autobpm", "-q"],
        ["autobpm", "-f", "-q"],
        ["ls", "-f", "$title|$bpm"],
        ["modify", "-y", "bpm!"],
        ["autobpm"],
        ["ls", "-f", "$title|$bpm"],
        ["autobpm", "title:Click 120"],
        ["ls", "-f", "$title|$bpm"],
    ]),
]

# importfeeds writes into the library (its default `dir`) or the music
# folder, which the art phase's state snapshot lists: m3u files byte for
# byte, symlinks by target, and the echo format on stdout.
ART_CASES += [
    ("importfeeds", "tracks", {},
     "{{plugins: [importfeeds], importfeeds: {{formats: [m3u, link, echo]}}}}",
     [
        ["import", "-A", "-q", "{music}"],
        ["ls", "-f", "$title|$path"],
        ["import", "-A", "-q", "{music}"],
    ]),
    ("importfeeds-dir", "tracks", {},
     "{{plugins: [importfeeds], importfeeds: {{formats: [m3u, link], dir:"
     " '{root}/music'}}}}",
     [
        ["import", "-A", "-q", "{music}"],
        ["ls", "-f", "$title|$path"],
    ]),
    ("importfeeds-absolute", "tracks", {},
     "{{plugins: [importfeeds], importfeeds: {{formats: [m3u, echo],"
     " absolute_path: yes}}}}",
     [
        ["import", "-A", "-q", "{music}"],
        ["import", "-A", "-q", "{music}"],
    ]),
]

# metasync's iTunes source against upstream beets' test fixture (two
# tracks of the art corpus, one with `file://localhost` and percent
# escapes), copied into the case's music folder with the paths filled in.
_ITUNES_FIXTURE = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "fixtures", "itunes_library.xml",
)

ART_CASES += [
    ("metasync", "tracks", {"itunes.xml": ("copy", _ITUNES_FIXTURE)},
     "{{plugins: [metasync], metasync: {{source: itunes, itunes: {{library:"
     " '{root}/music/itunes.xml'}}}}}}",
     [
        ["import", "-A", "-q", "{music}"],
        ["metasync", "-p"],
        ["metasync"],
        ["ls", "-f", "$title|$itunes_rating|$itunes_playcount|$itunes_skipcount"
                     "|$itunes_lastplayed|$itunes_lastskipped|$itunes_dateadded"],
        ["metasync"],
        ["metasync", "-s", "itunes", "-f", "$title: $itunes_rating", "Two"],
    ]),
]
