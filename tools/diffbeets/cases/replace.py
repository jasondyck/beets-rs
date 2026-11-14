"""Diffbeets cases: the 5.22 `replace` plugin.

`beet replace [-l N] QUERY NEW_FILE` moves another audio file over a
library item's file, keeping the item's tags in the database. The cases use
the `replace/` art corpus (an importable album plus replacement files with
other tags, another format and a text file) and answer the menu and
confirmation prompts; the output, the resulting files and the stored paths
are compared. The menu lists the item's field names, whose order beets takes
from a Python set: diffbeets sorts them (`sort_menu_keys`).
"""

ORDER = 97

ART_CASES = []
EXPECTED = {}

_COPY = {
    "new.mp3": ("copyfile", "{corpus}/replace/new.mp3"),
    "new2.mp3": ("copyfile", "{corpus}/replace/new2.mp3"),
    "new.flac": ("copyfile", "{corpus}/replace/new.flac"),
    "notes.txt": ("copyfile", "{corpus}/replace/notes.txt"),
}

ART_CASES += [
    ("replace", "replace", _COPY, "{{plugins: [replace]}}", [
        ["import", "-A", "-q", "{music}/album"],
        ["ls", "-f", "$title|$path|$length|$format"],
        # Same extension: the new file takes the old path; the item's
        # stored tags stay until `update` reads the new file's.
        (["replace", "Rep", "{music}/new.mp3"], "1\nyes\n"),
        ["ls", "-f", "$title|$path|$length|$format"],
        ["update", "-p"],
        ["update"],
        ["ls", "-f", "$title|$artist|$path|$length|$format"],
        # Confirmation declined, then another extension: the item's path
        # follows it and the old file is removed.
        (["replace", "New Title", "{music}/new.flac"], "1\nno\n"),
        (["replace", "Two", "{music}/new.flac"], "1\ny\n"),
        ["ls", "-f", "$title|$path|$format"],
        # Cancelled at the menu (0), then the argument and file checks.
        (["replace", "Two", "{music}/new2.mp3"], "0\n"),
        ["replace"],
        ["replace", "Two"],
        ["replace", "Two", "{music}/notes.txt"],
        ["replace", "Two", "{music}"],
        ["replace", "Nothing", "{music}/new2.mp3"],
        # A non-number, an out-of-range number, then cancel.
        (["replace", "Two", "{music}/new2.mp3"], "-1\nnope\n0\n"),
    ]),
    # `-l` limits the menu to the first match (`update` shows which item
    # was replaced).
    ("replace-limit", "replace", _COPY, "{{plugins: [replace]}}", [
        ["import", "-A", "-q", "{music}/album"],
        (["replace", "-l", "1", "Rep", "{music}/new.mp3"], "0\n"),
        (["replace", "-l", "1", "Rep", "{music}/new.mp3"], "1\ny\n"),
        ["update"],
        ["ls", "-f", "$title|$path|$length"],
    ]),
]
