"""Diffbeets cases: importplugins."""

ORDER = 80

from .art import _IMP
from .art import _LSA

ART_CASES = []
IMPORT_CASES = []

# ---------------------------------------------------------------------------
# 5.18b: import/file plugins.

# importadded: the source mtimes become `added` (copytree keeps them, so the
# two tools see the same values), and with preserve_mtimes the item's mtime.
IMPORT_CASES += [
    ("importadded", "fx", ["-A", "-C"],
     "plugins: [musicbrainz, importadded]\n", []),
    ("importadded-preserve", "fx", ["-A", "-C"],
     "plugins: [musicbrainz, importadded]\nimportadded:\n    preserve_mtimes: yes\n", []),
    ("importadded-singletons", "fx", ["-A", "-C", "-s"],
     "plugins: [musicbrainz, importadded]\n", []),
    # filefilter: only the matching files are imported (as-is: beets filters
    # the task's items at import_task_created).
    ("filefilter-asis", "fx", ["-A", "-C"],
     "plugins: [musicbrainz, filefilter]\nfilefilter:\n    path: '.*1\\.mp3$'\n", []),
    ("filefilter-none", "fx", ["-A", "-C"],
     "plugins: [musicbrainz, filefilter]\nfilefilter:\n    path: nothing\n", []),
    ("filefilter-singletons", "fx", ["-A", "-C", "-s"],
     "plugins: [musicbrainz, filefilter]\nfilefilter:\n    singleton_path: '.*2\\.mp3$'\n", []),
    # ihate: an applied match matching `skip` is skipped instead (the
    # listener changes the choice), `warn` only logs (at WARNING here).
    ("ihate-skip-album", "fx", ["-t", "-C"],
     "plugins: [musicbrainz, ihate]\nihate:\n    skip: ['album:Fixture Album']\n", ["a"]),
    ("ihate-skip-artist", "fx", ["-t", "-C"],
     "plugins: [musicbrainz, ihate]\nihate:\n    skip: ['artist:Fixture Artist']\n", ["a"]),
    ("ihate-warn", "fx", ["-t", "-C"],
     "plugins: [musicbrainz, ihate]\nihate:\n    warn: ['artist:Fixture Artist']\n", ["a"]),
    ("ihate-no-match", "fx", ["-t", "-C"],
     "plugins: [musicbrainz, ihate]\nihate:\n    skip: ['album:not this']\n", ["a"]),
    # badfiles: the checker's output aborts an autotagged import.
    ("badfiles-abort", "fx", ["-t", "-C"],
     "plugins: [musicbrainz, badfiles]\nbadfiles:\n    check_on_import: yes\n"
     "    import_action_on_error: abort\n    commands:\n"
     "        mp3: 'sh -c \"echo error: broken; exit 1\"'\n", []),
    ("badfiles-skip", "fx", ["-t", "-C"],
     "plugins: [musicbrainz, badfiles]\nbadfiles:\n    check_on_import: yes\n"
     "    import_action_on_error: skip\n    commands:\n"
     "        mp3: 'sh -c \"echo error: broken; exit 1\"'\n", []),
]

# The commands and hooks, on fresh libraries (art phase).
_HOOK = ("{{plugins: [hook], hook: {{hooks: ["
         "{{event: album_imported, command: 'echo album-imported {{album}}'}},"
         "{{event: write, command: 'echo write {{event}} {{item.title}} {{item.album}}'}},"
         "{{event: after_write, command: 'echo after {{path}}'}}"
         "]}}}}")
_HOOK_FAIL = "{{plugins: [hook], hook: {{hooks: [{{event: write, command: /bin/false}}]}}}}"
_BADFILES = "{{plugins: [badfiles], badfiles: {{commands: {{mp3: /bin/false}}}}}}"
_BADFILES_OK = "{{plugins: [badfiles], badfiles: {{commands: {{mp3: /bin/true}}}}}}"
_ADD = "{{plugins: [importadded], importadded: {{preserve_mtimes: yes}}}}"
_UNIMPORTED = ("{{plugins: [unimported], directory: '{{root}}/music'}}")
_UNIMPORTED_IGNORE = ("{{plugins: [unimported], directory: '{{root}}/music',"
                      " unimported: {{ignore_extensions: [mp3]}}}}")
_ADD_LS = ["ls", "-f", "$title|$added|$mtime"]
_ADD_LSA = ["ls", "-a", "-f", "$album|$added"]

ART_CASES += [
    ("hook", "tracks", {}, _HOOK, [_IMP, ["write", "-f"], _LSA]),
    ("hook-fail", "tracks", {}, _HOOK_FAIL, [_IMP, ["write", "-f"], _LSA]),
    ("importadded", "tracks", {}, _ADD, [_IMP, _ADD_LS, _ADD_LSA]),
    ("importadded-singletons", "tracks", {}, _ADD,
     [["import", "-A", "-q", "-s", "{music}"], _ADD_LS]),
    # beets checks a query's files with par_map (their output order is
    # arbitrary), so each step names one file.
    ("badfiles", "tracks", {}, _BADFILES,
     [_IMP, ["bad", "-l", "1"], ["bad", "-v", "-l", "1", "title:One"]]),
    ("badfiles-ok", "tracks", {}, _BADFILES_OK,
     [_IMP, ["bad", "-l", "1"], ["bad", "-v", "-l", "1", "title:Two"]]),
    ("unimported", "tracks", {}, _UNIMPORTED, [_IMP, ["unimported"]]),
    ("unimported-ignore", "tracks", {}, _UNIMPORTED_IGNORE, [_IMP, ["unimported"]]),
]
