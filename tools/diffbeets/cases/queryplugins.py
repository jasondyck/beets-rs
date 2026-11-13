"""Diffbeets cases: the query and template plugins (5.20b).

fuzzy (`~` prefix), bareasc (`#` prefix), limit (`<N` prefix and `lslimit`)
and titlecase (`%titlecase{}`). All of these are read-only: the `titlecase`
command is covered by unit tests (it rewrites the library).
"""

ORDER = 95

from ._corpus import ROOT

EXPECTED = {}

EXTRA_CONFIGS = {
    "queryplugins.yaml": "plugins: [fuzzy, bareasc, limit, titlecase]\n",
    "queryplugins-fuzzy.yaml": "plugins: [fuzzy]\nfuzzy:\n    threshold: 0.4\n",
    "queryplugins-titlecase.yaml": (
        "plugins: [titlecase]\ntitlecase:\n"
        "    preserve: [ABBA, easyFun, 'THE PSYCHIC ED RUSH', \"D'Angelo\"]\n"
        "    separators: ['/']\n"
        "    replace: [[\"'\", \"'\u2019\"]]\n"
    ),
    "queryplugins-titlecase-flags.yaml": (
        "plugins: [titlecase]\ntitlecase:\n"
        "    small_first_last: no\n"
        "    force_lowercase: yes\n"
    ),
}

_QP = ["-c", f"{ROOT}/queryplugins.yaml"]
_QF = ["-c", f"{ROOT}/queryplugins-fuzzy.yaml"]
_QT = ["-c", f"{ROOT}/queryplugins-titlecase.yaml"]
_QTF = ["-c", f"{ROOT}/queryplugins-titlecase-flags.yaml"]

CASES = [
    # fuzzy: the pattern is the value after the prefix, smartcase and the
    # threshold adjusted for shorter patterns.
    ("qp fuzzy title", [*_QP, "ls", "-f", "$title", "title:~one"]),
    ("qp fuzzy title case", [*_QP, "ls", "-f", "$title", "title:~One"]),
    ("qp fuzzy artist", [*_QP, "ls", "-f", "$title", "artist:~artist"]),
    ("qp fuzzy album", [*_QP, "ls", "-a", "-f", "$album", "album:~album"]),
    ("qp fuzzy nothing", [*_QP, "ls", "-f", "$title", "title:~zzzzzz"]),
    ("qp fuzzy low threshold", [*_QF, "ls", "-f", "$title", "title:~xyzzy"]),
    ("qp fuzzy negated", [*_QP, "ls", "-f", "$title", "--", "-title:~one"]),
    # bareasc: unidecoded, smartcase substring matching.
    ("qp bareasc artist", [*_QP, "ls", "-f", "$title", "artist:#beyonce"]),
    ("qp bareasc case", [*_QP, "ls", "-f", "$title", "artist:#Beyonce"]),
    ("qp bareasc title", [*_QP, "ls", "-f", "$title", "title:#cafe"]),
    ("qp bareasc album", [*_QP, "ls", "-a", "-f", "$album", "album:#cafe"]),
    ("qp bareasc negated", [*_QP, "ls", "-f", "$title", "--", "-artist:#beyonce"]),
    # bareasc's command prints the default format, unidecoded (it takes
    # -a/-p/-f like `list`; -f and -p change the default format).
    ("qp bareasc command", [*_QP, "bareasc"]),
    ("qp bareasc -a", [*_QP, "bareasc", "-a"]),
    ("qp bareasc -f", [*_QP, "bareasc", "-f", "$artist|$title", "artist:#beyonce"]),
    ("qp bareasc -p", [*_QP, "bareasc", "-p", "title:#cafe"]),
    # limit: the stateful `<N` prefix (head only) and `lslimit`.
    ("qp limit", [*_QP, "ls", "-f", "$title", "<2"]),
    ("qp limit zero", [*_QP, "ls", "-f", "$title", "<0"]),
    ("qp limit one", [*_QP, "ls", "-f", "$title", "<1"]),
    ("qp limit field", [*_QP, "ls", "-f", "$title", "title:<2"]),
    ("qp limit album", [*_QP, "ls", "-a", "-f", "$album", "<2"]),
    ("qp lslimit head", [*_QP, "lslimit", "--head", "2"]),
    ("qp lslimit tail", [*_QP, "lslimit", "--tail", "2"]),
    ("qp lslimit head format", [*_QP, "lslimit", "--head", "2", "-f", "$title"]),
    ("qp lslimit tail path", [*_QP, "lslimit", "--tail", "1", "-p"]),
    ("qp lslimit album", [*_QP, "lslimit", "-a", "--head", "2"]),
    ("qp lslimit limit", [*_QP, "lslimit", "-l", "1", "--tail", "2"]),
    ("qp lslimit both", [*_QP, "lslimit", "--head", "1", "--tail", "1"]),
    # titlecase: the template function, with the default and configured
    # preserve/separators/replace/small_first_last/force_lowercase.
    ("qp titlecase", [*_QP, "ls", "-f", "%titlecase{the beatles and the beach boys}"]),
    ("qp titlecase caps", [*_QP, "ls", "-f", "%titlecase{ALL CAPS TITLE}"]),
    ("qp titlecase name", [*_QP, "ls", "-f", "%titlecase{d'angelo and the vanguard}"]),
    ("qp titlecase slash", [*_QP, "ls", "-f", "%titlecase{a/b/c}"]),
    ("qp titlecase preserve", [*_QT, "ls", "-f", "%titlecase{abba and easyfun}"]),
    ("qp titlecase phrase", [*_QT, "ls", "-f", "%titlecase{the psychic ed rush}"]),
    ("qp titlecase separators", [*_QT, "ls", "-f", "%titlecase{it / the test}"]),
    ("qp titlecase replace", [*_QT, "ls", "-f", "%titlecase{don't stop}"]),
    ("qp titlecase flags", [*_QTF, "ls", "-f", "%titlecase{THE BEATLES}"]),
    ("qp titlecase flags small", [*_QTF, "ls", "-f", "%titlecase{In a Silent Way}"]),
]

# beets raises an uncaught ValueError (a traceback, exit 1) when both are
# given; beets-rs reports it as an error message.
EXPECTED["qp lslimit both"] = (
    "beets' plugin raises an uncaught ValueError (traceback); beets-rs "
    "prints an error message (both exit 1)"
)
