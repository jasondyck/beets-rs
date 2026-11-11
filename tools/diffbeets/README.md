# diffbeets: beets-rs against reference beets

```sh
~/.cache/beets-reference/venv/bin/python tools/diffbeets/diffbeets.py
```

It builds `target/release/beet` (skip with `--no-build`), then in a fresh temp
dir (under `$TMPDIR`, or `--workdir DIR`):

1. generates a corpus of 1-second audio files with ffmpeg (`src/`): unicode,
   `½`, slashes, quotes, `$ % { } ,` in titles, empty fields, a multi-disc
   album in `CD1`/`CD2`, a VA album, same album name by different artists and
   years, long names, leading dots, singletons, and one track per format
   and codec with odd lengths (MP3 CBR/VBR/no Xing header, FLAC, AAC, ALAC,
   Ogg Vorbis, Opus, WavPack, AIFF, WAV) for exact `length`/`bitrate`;
2. writes the same `config.yaml` (and the extra configs used with `-c`) to
   `ref/` and `rs/`, differing only in `directory:` (`ref/lib`, `rs/lib`);
3. imports every directory as-is (`import -A -q`, copying) and applies the
   same `modify` commands with each tool: reference beets via
   `~/.cache/beets-reference/beet` with `BEETSDIR=<tmp>/ref`, beets-rs with
   `BEETSRSDIR=<tmp>/rs`;
4. runs every case with both, from inside the library directory, and compares
   stdout, stderr and exit status, with each tool's own temp root replaced by
   `<ROOT>`.

The report lists each case that isn't a plain PASS (`--show-pass` for all):
`DIFF` (with both outputs, or a unified diff when long), `EXPECTED` (a known
difference: the reason is printed) and `FIXED?` (listed as expected but now
equal: remove it from the list). The exit status is 1 if anything is `DIFF`
or `FIXED?`.

Cases, corpus, config and the expected differences live in `cases.py`:
queries (run with `ls` and `ls -a`) from beets' `test_query.py`,
`test_queryparse.py`, `test_datequery.py` and `test_sort.py`; templates
(`ls -f`, `ls -a -f`) from `test_functemplate.py` and `test_library.py`; path
formats and path options (`mv -p -d`). Relative date cases only use ranges that
are robust to when the run happens.

Options: `--only SUBSTR` (repeatable) runs matching case ids, `--keep` keeps
the temp dir, `-j N` sets parallelism, `-v` lists the setup steps.
