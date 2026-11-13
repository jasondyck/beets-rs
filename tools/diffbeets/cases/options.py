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
    # Option values, checked the way optparse's type/callback checks do
    # (2.11): integers, enumeration choices, missing values, and the
    # non-negative check of beets' add_limit_option callback.
    ["ls", "-la"],
    ["ls", "-l", "a"],
    ["ls", "--limit=a"],
    ["ls", "--lim", "a"],
    ["ls", "-l", "-1"],
    ["ls", "--limit=-1"],
    ["ls", "-l", " 3 "],
    ["modify", "-l", "x"],
    ["rm", "-l", "x"],
    ["stats", "-l", "x"],
    ["-p", "export", "export", "-f", "yaml"],
    ["-p", "export", "export", "--format=yaml"],
    ["-p", "smartplaylist", "splupdate", "--output=xspf"],
    ["-p", "smartplaylist", "splupdate", "--output"],
    ["-p", "info", "info", "--limit=-1"],
    ["-p", "info", "info", "--limit", "x"],
    ["-p", "info", "info", "-l", "-1"],
    ["-p", "duplicates", "duplicates", "-l", "x"],
    ["-p", "missing", "missing", "-l", "x"],
    ["-p", "random", "random", "-l", "x"],
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
