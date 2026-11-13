# Third-party notices

beets-rs is released under the MIT License (see `LICENSE`), like beets. **Read
the first section first**: some files follow GPL-licensed mutagen code, which
needs a licensing decision before redistribution. The rest of this file lists
the third-party projects whose code or data beets-rs ports or embeds, with the
upstream licence and where it lives in this tree. It is an inventory, not a
change of licence: the notices below are the ones the upstream licences require
us to keep. Rust crate dependencies (crates.io, pinned in `Cargo.lock`) are not
"embedded" in the tree; their licences ship in the crates themselves — see
"Rust dependencies" at the end for the non-permissive ones.

## ⚠ Copyleft-derived code — needs a decision

**mutagen — GPL-2.0-or-later** (<https://mutagen.readthedocs.io>, GPLv2 text in
mutagen's `COPYING`). Its audio-property parsers are followed closely, name by
name, in:

- `src/props.rs` — re-implements mutagen's `info.length` / `info.bitrate` per
  format from mutagen's own code: `MPEGInfo` and `_parse_vbr_header` (Xing/Info,
  VBRI), `LAMEHeader.parse_version` and `guess_settings`, `FLAC`,
  `MP4Info`, `OggVorbisInfo`/`OggOpusInfo` and `OggPage.find_last`,
  `WavPackInfo`, `MonkeysAudioInfo`, `WaveStreamInfo`, AiffInfo. The comments
  cite each class/method; the parsing logic (not just the file-format specs)
  is translated from mutagen.

These parts should be treated as derived from a GPL-2.0-or-later work. Until
that is decided, the project as a whole cannot honestly be distributed under
the MIT licence alone if these files remain as they are. Options:

1. **Keep and relicense**: beets-rs (or these files) under GPL-2.0-or-later,
   the same licence mutagen uses (beets itself is MIT but only *depends* on
   mutagen, so this is a change for us, not a beets-compatibility issue).
2. **Rewrite from the specifications**: implement `src/props.rs` from the
   MPEG audio / Xing / LAME / VBRI, FLAC, ISO-BMFF (`mdhd`, `stsd`),
   Ogg (granule position), WavPack, Monkey's Audio and WAV/AIFF format
   documentation instead of mutagen's code; take the ID3v1 genre list from
   the ID3v1/ID3v2 specification (or a permissively licensed crate such as
   `id3`) and re-implement `TCON` conversion / ID3v1 saving from the ID3
   spec. Exactness against beets can be re-checked with the existing
   `props`/`mediafile` tests and diffbeets tag round-trips.
3. **Replace with permissive crates**: e.g. the `id3` crate for ID3v1
   genres/`TCON`, and accept lofty's `length`/`bitrate` (this changes user-
   visible `length`/`bitrate` values vs beets, so it conflicts with the
   port's compatibility goal).

Not copyleft, but worth knowing: `tools/diffbeets` and the `tests/fixtures/mb/
expected/` data were *compared against* reference beets in a venv that has
mutagen installed; mutagen is never embedded or shipped, and no mutagen code
is used at build or run time.

**Not copyleft-derived (checked):**

- **`src/mediafile.rs` tag rules** — rewritten from the format specifications
  rather than from mutagen: the 192 ID3v1/Winamp genre names (`ID3V1_GENRES`,
  `WINAMP_GENRES`) from the ID3v1 genre list and Winamp's published extension
  list (the combined table the ID3v2 specifications refer to as the ID3v1
  genre list); `TCON` parsing from the
  ID3v2.3/2.4 “Content type” sections (§4.2.2 in both, including parenthesised
  genre references, `RX`/`CR` and the escaped leading parenthesis); the ID3v1
  tag build/merge from the ID3v1 and ID3v1.1 tag layouts (30-character fields,
  v1.1 track in the comment field, one-byte genre); ID3v2.3 multi-value
  conversion (NUL-separated v2.4 values to "/"-joined v2.3 strings, ID3v2.3
  §4.2; date frames TDRC→TYER/TDAT/TIME and TDOR→TORY per v2.4 Appendix A,
  applied by the tag writer); and the iTunes `trkn`/`disk`/`tmpo` atom layouts
  from Apple's iTunes metadata format. Behaviour was checked against beets
  2.14.1 black-box (reading and writing all genres, TCON forms, ID3v1 and
  ID3v2.3 tags, MP4 atoms); no mutagen source was used.
- **`src/plugins/scrub.rs`** — behaviour-following only: the per-format tag
  stripping is implemented through lofty; no mutagen code is copied.

- **Unidecode — GPL-2.0-or-later**: not embedded. beets uses Python
  Unidecode; beets-rs uses the `deunicode` crate (BSD-3-Clause) in `src/template.rs`,
  `src/plugins/bareasc.rs`, the search-query builders and the lyrics
  backends. No Unidecode table was copied (`bareasc.rs` documents the known
  difference for vulgar fractions).
- **Chromaprint — LGPL-2.1**: only the *algorithm* (fingerprint comparison)
  is reproduced, following pyacoustid's MIT-licensed Python
  implementation (see below); no Chromaprint C/C++ code is copied.
- **python-mpd2 — LGPL-3.0**: not embedded. `src/plugins/bpd/` and
  `src/plugins/mpdstats.rs` speak the MPD text protocol directly (beets'
  bpd/mpdstats plugins use python-mpd2).
- **jQuery 1.7.1** is dual-licensed MIT / GPL-2.0; the MIT option is used
  (as beets does).

## Ported code and copied data (permissive)

| Component | Upstream licence | Where in beets-rs | What was ported/copied |
|---|---|---|---|
| beets 2.14.1 | MIT | most of `src/*.rs` and `src/plugins/` | Every module is a port of the matching beets module (`/*!` headers name it); `src/query/`, `src/template.rs`, `src/paths.rs`, `src/library.rs`, `src/importer.rs`, `src/autotag.rs`, `src/mb.rs`, `src/hooks.rs`, `src/display.rs`, `src/layout.rs`, the plugin modules, etc. |
| beets 2.14.1 | MIT | `src/config.rs` (`DEFAULT_CONFIG`) | beets' `config_default.yaml` (keys, values and comments) |
| beets 2.14.1 | MIT | `src/subcommand_help.rs` | beets' `beet CMD -h` usage/option texts, captured from reference beets |
| beets 2.14.1 | MIT | `src/plugins/lastgenre/{genres.txt,genres-tree.yaml,aliases.yaml}` | beets' bundled genre whitelist, canonicalisation tree and alias table |
| beets 2.14.1 | MIT | `src/plugins/web/{static/beets.js,static/beets.css,templates/index.html}` | beets' web UI assets (see also the vendored JS below) |
| beets 2.14.1 | MIT | `tests/fixtures/{audio/bpm.mp3,web/full.mp3,metasync/itunes_library_*.xml,lyrics/*.html}` | byte-identical copies of beets' `test/rsrc` files |
| beets 2.14.1 | MIT | `tools/diffbeets/`, `src/query/tests.rs`, many `src/plugins/*_tests.rs` | cases, fixtures and expected values taken/adapted from beets' tests |
| MediaFile 0.17 | MIT | `src/mediafile.rs`, `src/media.rs`, `src/plugins/art.rs` (`image_mime_type`, `image_extension`) | MediaFile's tag field table, read/write styles and order, value handling |
| MediaFile 0.17 | MIT | `src/media.rs` | MediaFile's per-format tag classes as used by beets |
| filetype 1.2 | MIT (Tomás Aparicio) | `src/plugins/art.rs` | `image_mime_type`/`image_extension` magic-byte mapping (used by MediaFile/beets art handling) |
| confuse 2.3.1 | MIT | `src/config.rs` | config layering, `config.add` defaults handling and `beet config` dump layout |
| PyYAML 6.0 | MIT | `src/pyyaml.rs` | `SafeRepresenter`/`Resolver`/`SafeConstructor` and `safe_dump_all`/`safe_load_all` semantics |
| libyaml (via the `unsafe-libyaml` crate) | MIT | `src/pyyaml.rs` | YAML event emitter/parser (the crate is libyaml translated to Rust) |
| python-titlecase 2.4.1 | MIT | `src/plugins/titlecase.rs` | `titlecase()` and its regexes as used by beets' titlecase plugin |
| soco 0.32.2 | MIT | `src/plugins/sonos.rs` | SSDP discovery (`any_soco`/`discover`), `PLAYER_SEARCH`, `RefreshShareIndex` envelope, SOAP fault handling |
| pyacoustid 1.3.1 | MIT | `src/plugins/chroma.rs` | `fingerprint_file`, `_api_request`, AcoustID lookup/submit and fingerprint comparison used by `beet chromasearch` |
| urllib3 2.x | MIT | `src/plugins/requests.rs` | `Retry`/backoff/rate-limit behaviour of beets' `TimeoutAndRetrySession` |
| requests 2.x | Apache-2.0 | `src/plugins/requests.rs` | `Session`/`raise_for_status`/`HTTPError` message semantics |
| pylast 7.2 | Apache-2.0 | `src/plugins/lastgenre/client.rs`, `src/plugins/lastimport.rs` | `LastFMNetwork`'s request form, `get_album`/`get_artist`/`get_track`/`get_top_tags` and the XML extraction |
| discogs_client 2.10 | BSD-2-Clause | `src/plugins/discogs.rs` | the `Client`/`Release`/`Master`/`Search` API shape and error behaviour (the client itself is written from the HTTP API; no discogs_client code was copied) |
| librosa 1.x | ISC | `src/plugins/autobpm/librosa.rs` | `load`, `onset_strength`, `power_to_db`, `melspectrogram` and `beat_track`'s tempo estimate |
| resampy 0.4 | ISC | `src/plugins/autobpm/librosa.rs` | the `kaiser_fast` resampling filter |
| Python 3 `difflib` | PSF-2.0 | `src/diff.rs` | `SequenceMatcher`/`get_opcodes` (and the `colordiff` usage of beets) |
| ImageMagick | ImageMagick License | `src/artresizer.rs` | the `PHASH` similarity metric (re-implemented in Rust; see "Attribution-only" below) |
| jQuery 1.7.1 + Sizzle | MIT (or GPL-2.0, MIT chosen) | `src/plugins/web/static/jquery.js` | as bundled with beets |
| Underscore.js 1.2.2 | MIT | `src/plugins/web/static/underscore.js` | as bundled with beets |
| Backbone.js 0.5.3 | MIT | `src/plugins/web/static/backbone.js` | as bundled with beets |

Followed for behaviour only, with no upstream code copied (no notice beyond
attribution): Python's `shlex.split`/`shlex.quote` (`src/plugins/convert.rs`,
`src/plugins/duplicates.rs`, `src/plugins/smartplaylist.rs`), Python's
`str.format` (`src/plugins/hook.rs`), Python expression/statement semantics
(`src/plugins/inline/`, a subset evaluator), the MPD protocol
(`src/plugins/bpd/`, `src/plugins/mpdstats.rs`), the iTunes XML plist
(`src/plugins/metasync.rs`), Flask/Werkzeug's HTTP responses
(`src/plugins/web.rs`; Flask and Werkzeug are BSD-3-Clause), and
numpy/scipy/ffmpeg behaviour behind
librosa's functions (our `autobpm` uses `rustfft`).

Not embedded and not ported: beets' optional `audiotools` (GPL-3.0) and
`gstreamer` replaygain backends (beets-rs' replaygain plugin uses ffmpeg, a
command, or metaflac), `langdetect` (lyrics language detection), and PIL (our
`artresizer` follows ImageMagick, see above). External programs that beets and
beets-rs *invoke* at run time — `ffmpeg`, `fpcalc`, `mp3gain`, `metaflac`,
`keyfinder`, `mp3val`, `flac` — are not part of this tree; they are the
user's (or a binary distributor's) own dependencies.

### Attribution-only (algorithm provenance, no code copied)

- **ImageMagick's `PHASH` metric** — `src/artresizer.rs` re-implements the
  comparison metric to match beets' ImageMagick backend. ImageMagick Studio
  LLC, ImageMagick License: <https://imagemagick.org/license/>.
- **Chromaprint fingerprint comparison** — `src/plugins/chroma.rs` follows
  pyacoustid's MIT implementation of the comparison; the format and the
  algorithm originate in Chromaprint (LGPL-2.1, <https://acoustid.org/chromaprint>).

## Required notices

### beets — MIT

```
The MIT License

Copyright (c) 2010-2016 Adrian Sampson

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

### MediaFile — MIT

```
The MIT License

Copyright (c) 2015 Adrian Sampson

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

### filetype — MIT

```
The MIT License (MIT)

Copyright (c) 2016 Tomás Aparicio

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### confuse — MIT

```
The MIT License

Copyright (c) 2015 Adrian Sampson

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

### PyYAML — MIT

```
Copyright (c) 2017-2021 Ingy döt Net
Copyright (c) 2006-2016 Kirill Simonov

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### libyaml / unsafe-libyaml — MIT

The `unsafe-libyaml` crate is libyaml translated from C to Rust (by dtolnay,
using c2rust); the crate's `LICENSE-MIT` is the MIT permission text, and
libyaml's `License` is:

```
Copyright (c) 2017-2020 Ingy döt Net
Copyright (c) 2006-2016 Kirill Simonov

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

(<https://github.com/yaml/libyaml>)

### python-titlecase — MIT

```
The MIT License (MIT)

Copyright (c) 2021 Patrick William Pannuto

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### SoCo — MIT

```
The MIT License (MIT)
=====================

Copyright (c) Rahim Sonawalla and individual contributors (see AUTHORS)

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

### pyacoustid — MIT

```
The MIT License

Copyright (c) 2011-2019 Adrian Sampson

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

### urllib3 — MIT

```
MIT License

Copyright (c) 2008-2020 Andrey Petrov and contributors.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### jQuery 1.7.1 / Sizzle — MIT (dual-licensed with GPL-2.0; MIT chosen)

```
Copyright 2016, John Resig
Copyright 2016, The Dojo Foundation (Sizzle)

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

### Underscore.js 1.2.2 — MIT

```
Copyright (c) 2011 Jeremy Ashkenas, DocumentCloud Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

### Backbone.js 0.5.3 — MIT

```
Copyright (c) 2010 Jeremy Ashkenas, DocumentCloud Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

### librosa — ISC

```
## ISC License

Copyright (c) 2013--2026, librosa development team.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

### resampy — ISC

```
ISC License

Copyright (c) 2016, Brian McFee

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

### discogs_client — BSD-2-Clause

API compatibility only (the client was written from the HTTP API; no
discogs_client code was copied). Kept because the module documents following
its API shape:

```
Copyright © 2015 Discogs. All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

  1. Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

  2. Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY DISCOGS ''AS IS'' AND ANY EXPRESS OR IMPLIED
WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT
SHALL DISCOGS OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR
BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING
IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY
OF SUCH DAMAGE.

The views and conclusions contained in the software and documentation are those
of the authors and should not be interpreted as representing official policies,
either expressed or implied, of Discogs.
```

### pylast and requests — Apache-2.0

`src/plugins/lastgenre/client.rs`, `src/plugins/lastimport.rs` follow pylast
(Apache-2.0), and `src/plugins/requests.rs` follows `requests`
(Apache-2.0) semantics. Full licence:

```
                                 Apache License
                           Version 2.0, January 2004
                        https://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS

   APPENDIX: How to apply the Apache License to your work.

      To apply the Apache License to your work, attach the following
      boilerplate notice, with the fields enclosed by brackets "{}"
      replaced with your own identifying information. (Don't include
      the brackets!)  The text should be enclosed in the appropriate
      comment syntax for the file format. We also recommend that a
      file or class name and description of purpose be included on the
      same "printed page" as the copyright notice for easier
      identification within third-party archives.

   Copyright {yyyy} {name of copyright owner}

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       https://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
```

### Python 3 `difflib` — PSF-2.0

`src/diff.rs` is a Rust translation of Python 3's `difflib.SequenceMatcher`
(as used by beets' `colordiff`): `longest_match`/`matching_blocks`/opcodes
were rewritten in Rust; there is no Python code left. Summary of changes:
translated to Rust, limited to character sequences and the operations beets'
change display needs.

```
Copyright (c) 2001-2014 Python Software Foundation; All Rights Reserved

PYTHON SOFTWARE FOUNDATION LICENSE VERSION 2
--------------------------------------------

1. This LICENSE AGREEMENT is between the Python Software Foundation
("PSF"), and the Individual or Organization ("Licensee") accessing and
otherwise using this software ("Python") in source or binary form and
its associated documentation.

2. Subject to the terms and conditions of this License Agreement, PSF
hereby grants Licensee a nonexclusive, royalty-free, world-wide
license to reproduce, analyze, test, perform and/or display publicly,
prepare derivative works, distribute, and otherwise use Python alone
or in any derivative version, provided, however, that PSF's License
Agreement and PSF's notice of copyright, i.e., "Copyright (c) 2001,
2002, 2003, 2004, 2005, 2006, 2007, 2008, 2009, 2010, 2011, 2012,
2013, 2014 Python Software Foundation; All Rights Reserved" are
retained in Python alone or in any derivative version prepared by
Licensee.

3. In the event Licensee prepares a derivative work that is based on
or incorporates Python or any part thereof, and wants to make
the derivative work available to others as provided herein, then
Licensee hereby agrees to include in any such work a brief summary of
the changes made to Python.

4. PSF is making Python available to Licensee on an "AS IS"
basis.  PSF MAKES NO REPRESENTATIONS OR WARRANTIES, EXPRESS OR
IMPLIED.  BY WAY OF EXAMPLE, BUT NOT LIMITATION, PSF MAKES NO AND
DISCLAIMS ANY REPRESENTATION OR WARRANTY OF MERCHANTABILITY OR FITNESS
FOR ANY PARTICULAR PURPOSE OR THAT THE USE OF PYTHON WILL NOT
INFRINGE ANY THIRD PARTY RIGHTS.

5. PSF SHALL NOT BE LIABLE TO LICENSEE OR ANY OTHER USERS OF PYTHON
FOR ANY INCIDENTAL, SPECIAL, OR CONSEQUENTIAL DAMAGES OR LOSS AS
A RESULT OF MODIFYING, DISTRIBUTING, OR OTHERWISE USING PYTHON,
OR ANY DERIVATIVE THEREOF, EVEN IF ADVISED OF THE POSSIBILITY THEREOF.

6. This License Agreement will automatically terminate upon a material
breach of its terms and conditions.

7. Nothing in this License Agreement shall be deemed to create any
relationship of agency, partnership, or joint venture between PSF and
Licensee.  This License Agreement does not grant permission to use PSF
trademarks or trade name in a trademark sense to endorse or promote
products or services of Licensee, or any third party.

8. By copying, installing or otherwise using Python, Licensee
agrees to be bound by the terms and conditions of this License
Agreement.
```

## Rust dependencies

The build depends on crates.io packages pinned in `Cargo.lock`; each ships its
own licence with its sources. None carries copyleft obligations for how we
link it. The non-permissive ones are weak/file-level copyleft, unmodified:

- `symphonia` and its `symphonia-*` crates — **MPL-2.0** (audio decoding for
  `autobpm`).
- `scraper`'s `cssparser`, `cssparser-macros`, `selectors`, `dtoa-short` —
  **MPL-2.0** (HTML/CSS parsing for `fetchart`'s Wikipedia scraper and `web`).
- `option-ext` — **MPL-2.0** (via `dirs`).
- `deunicode` — BSD-3-Clause (transliteration, used instead of Unidecode).
- `r-efi` — MIT OR Apache-2.0 OR LGPL-2.1-or-later (UEFI target support,
  not used on Linux); the MIT option applies.

MPL-2.0 is file-level: since we do not modify those crates, distributing
beets-rs (source or binary) only requires that their source remain available
under MPL-2.0, which it is (crates.io/`Cargo.lock`).

If beets-rs binaries are distributed, the licences of *all* dependency crates
(MIT, Apache-2.0, BSD, ISC, MPL-2.0, ...) must be reproduced with them; this
file deliberately does not duplicate them — use the crates' own licence files
(e.g. `cargo about`/`cargo metadata` or the `.crate` archives) for that. The
notices above are for the code and data that is *copied into this tree*.
