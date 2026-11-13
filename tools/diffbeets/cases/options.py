"""Diffbeets cases: how optparse treats options (2.9).

Unknown options (beets' usage block, "no such option", exit 2), long-option
abbreviations, `--` ending option parsing, and option values that start with
a dash.
"""

ORDER = 110

_OPTION_CASES = [
    ["ls", "-y"],
    ["ls", "--bogus"],
    ["ls", "--bogus=x"],
    ["ls", "-aQ"],
    ["ls", "-Qa"],
    ["ls", "-artist:x"],
    ["ls", "-1"],
    ["ls", "---x"],
    ["ls", "--formatt"],
    ["ls", "--a"],
    ["ls", "--form"],
    ["ls", "--li", "1"],
    ["ls", "--="],
    ["ls", "-f", "-a"],
    ["ls", "-f", ""],
    ["ls", "-ap"],
    ["rm", "-y"],
    ["rm", "--bogus", "x"],
    ["mv", "-z", "x"],
    ["modify", "-y", "foo=1", "-q"],
    ["stats", "-Q"],
    ["config", "-Q"],
    ["fields", "--bogus"],
    ["-Q"],
    ["-Q", "ls"],
    ["--bogus"],
    ["ls", "--", "-artist:x"],
    ["ls", "--", "-1"],
    ["ls", "--", "--bogus"],
    ["ls", "--", "-a", "-f", "$title"],
    ["--", "ls", "-a"],
    ["--", "ls", "-y"],
    ["ls", "-l", "3"],
    ["ls", "--limit=2"],
    ["ls", "--lim", "2"],
    # Plugin commands (built by the registry at run time) get the same
    # treatment; `-p` enables the plugin so both tools know the command.
    ["-p", "fetchart", "fetchart", "-y"],
    ["-p", "fetchart", "fetchart", "--bogus"],
    # `-x` alone is a negated query on the nonexistent field `x`, so it
    # matches nearly every album: fetchart then downloads art for them
    # (network/timing dependent) and changes the shared library after the
    # main phase's outputs were recorded, which made the migrate:artpath
    # cases compare against stale expected output (and flaky). The extra
    # `album:...` term keeps the dash-leading argument (the point of the
    # case) while matching no album, so fetchart does nothing.
    ["-p", "fetchart", "fetchart", "-f", "--", "-x",
     "album:__diffbeets_no_such_album__"],
    ["-p", "info", "info", "-Q"],
    ["-p", "missing", "missing", "--nope"],
    ["-p", "fetchart,info", "info", "--bogus"],
]

CASES = [(f"cli-opt:{' '.join(args)}", args) for args in _OPTION_CASES]
