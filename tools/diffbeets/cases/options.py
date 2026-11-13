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
    ["-p", "fetchart", "fetchart", "-f", "--", "-x"],
    ["-p", "info", "info", "-Q"],
    ["-p", "missing", "missing", "--nope"],
    ["-p", "fetchart,info", "info", "--bogus"],
]

CASES = [(f"cli-opt:{' '.join(args)}", args) for args in _OPTION_CASES]
