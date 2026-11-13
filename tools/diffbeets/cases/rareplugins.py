"""Diffbeets cases: the rarely used 5.20 plugins that run offline:
mbsubmit, play (with `echo` as the player), keyfinder (its program is
missing: both tools log the same error) and autobpm (the librosa port on
click tracks; the library importer is compared too)."""

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
