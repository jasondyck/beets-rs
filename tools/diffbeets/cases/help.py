"""Diffbeets cases: `beet --help` (2.10).

beets lists the core and enabled plugins' commands sorted by name, with the
aliases in parentheses, in optparse's layout; `-h`/`help` do the same.
"""

ORDER = 105

# Plugins whose commands both tools' `--help` can list (reference beets'
# Python dependencies for the others aren't installed).
_CASES = [
    ["--help"],
    ["-h"],
    ["help"],
    ["--help", "ls"],
    ["-h", "badcmd"],
    ["--help", "--bogus"],
    ["-p", "fetchart", "--help"],
    ["-p", "fetchart,embedart,lyrics", "--help"],
    ["-p", "edit,info,missing,duplicates,ftintitle,zero", "--help"],
    ["-p", "smartplaylist,badfiles,unimported,web", "--help"],
    ["-p", "lastgenre,random,scrub,export", "--help"],
    ["-p", "bareasc,fish,thumbnails", "--help"],
    # 2.14: a command's own help, optparse's layout (usage, blank line,
    # Options table); `beet CMD -h` and `beet help CMD` alike.
    ["help", "import"],
    ["help", "move"],
    ["help", "fields"],
    ["help", "ls"],
    ["help", "badcmd"],
    ["ls", "-h"],
    ["ls", "--help"],
    ["import", "-h"],
    ["-p", "fetchart", "help", "fetchart"],
    ["-p", "fetchart,embedart", "help", "embedart"],
    ["-p", "fetchart", "fetchart", "-h"],
    ["-p", "duplicates,zero,missing,info,edit", "help", "zero"],
    ["-p", "smartplaylist,tidal", "help", "tidalsync"],
    # beets loads the enabled plugins before printing help: `limit` warns.
    ["-p", "limit", "help", "ls"],
    ["-p", "limit", "ls", "-h"],
]

CASES = [(f"cli-help:{' '.join(args)}", args) for args in _CASES]
