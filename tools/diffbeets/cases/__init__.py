"""Diffbeets' corpus, configurations and cases, one module per area.

Every module here is discovered automatically and may define `CASES` (the
main phase), `IMPORT_CASES`, `ART_CASES`, `EXPECTED` and `EXTRA_CONFIGS`;
`ORDER` controls their order (the corpus first). Adding an area module is
enough to add its cases to `tools/diffbeets/diffbeets.py`.
"""

import importlib
import pkgutil

from . import _corpus

_modules = []
for _info in pkgutil.iter_modules(__path__):
    if _info.name.startswith("_"):
        continue
    _modules.append(importlib.import_module(f"{__name__}.{_info.name}"))
_modules.sort(key=lambda m: m.ORDER)

CASES = []
IMPORT_CASES = []
ART_CASES = []
EXPECTED = {}
NORMALIZERS = {}
EXTRA_CONFIGS = _corpus.EXTRA_CONFIGS
for _m in _modules:
    _c = getattr(_m, "CASES", [])
    CASES += _c() if callable(_c) else _c
    IMPORT_CASES += getattr(_m, "IMPORT_CASES", [])
    ART_CASES += getattr(_m, "ART_CASES", [])
    EXPECTED.update(getattr(_m, "EXPECTED", {}))
    NORMALIZERS.update(getattr(_m, "NORMALIZERS", {}))
    EXTRA_CONFIGS.update(getattr(_m, "EXTRA_CONFIGS", {}))

# The shared corpus/config names, from whichever area module defines them.
CONFIG = _corpus.CONFIG
ROOT = _corpus.ROOT
CORPUS = _corpus.CORPUS
LAME_CORPUS = _corpus.LAME_CORPUS
IMPORTS = _corpus.IMPORTS
MODIFIES = _corpus.MODIFIES
for _name in ("IMPORT_CORPUS", "TAG_WRITES", "TAG_TEMPLATE", "THE_CONFIGS"):
    for _m in [_corpus, *_modules]:
        if hasattr(_m, _name):
            globals()[_name] = getattr(_m, _name)
            break


def cases():
    """The main phase's (case id, beet arguments) pairs."""
    return CASES
