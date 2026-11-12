"""Generates the richer MusicBrainz release fixtures in tests/fixtures/mb
(releases 3333... and 4444...), as the web service's JSON with beets'
includes: a Various Artists live compilation on two titled discs, with
multi-artist credits and join phrases, works with composers and lyricists,
arrangers and remixers, ISRCs, genres, tags, label info, release events
and URL relations; and a multi-artist promotional remix EP with "[no
label]", track numbers like "A1" and a year-and-month date.

    python3 tools/diffbeets/mbfixtures.py

Then regenerate the expected values with mbexpected.py.
"""
import json
import os

OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(
    os.path.abspath(__file__)))), "tests", "fixtures", "mb")


def artist(n, name, sort, aliases=()):
    return {"id": f"aaaaaaaa-0000-0000-0000-0000000001{n:02d}", "name": name,
            "sort-name": sort, "disambiguation": "", "type": "Person",
            "type-id": None, "country": None, "aliases": list(aliases)}


A = artist(1, "Artist A", "A, Artist",
           [{"name": "Künstler A", "sort-name": "A, Künstler", "locale": "de",
             "primary": True, "type": "Artist name", "type-id": None,
             "begin": None, "end": None, "ended": False}])
B = artist(2, "Artist B", "B, Artist")
C = artist(3, "Artist C", "C, Artist")
BAND = artist(4, "The Band", "Band, The")
COMP1 = artist(5, "Composer One", "One, Composer")
COMP2 = artist(6, "Composer Two", "Two, Composer")
LYR = artist(7, "Lyricist Lee", "Lee, Lyricist")
ARR = artist(8, "Arranger Ann", "Ann, Arranger")
REMIX = artist(9, "Remixer Rob", "Rob, Remixer")
VA = {"id": "89ad4ac3-39f7-470e-963a-56509c546377", "name": "Various Artists",
      "sort-name": "Various Artists", "disambiguation": "add compilation artist credits",
      "type": "Other", "type-id": None, "country": None, "aliases": []}


def credit(*parts):
    """parts: (artist, credited name, joinphrase)"""
    return [{"name": n, "joinphrase": j, "artist": a} for a, n, j in parts]


def rel_base(**kw):
    return {"direction": "forward", "begin": None, "end": None, "ended": False,
            "attributes": [], "attribute-ids": {}, "attribute-values": {},
            "source-credit": "", "target-credit": "", "type-id": "t", **kw}


def artist_rel(kind, a):
    return rel_base(**{"type": kind, "target-type": "artist", "artist": a})


def work_rel(wid, title, artist_rels):
    return rel_base(**{"type": "performance", "target-type": "work", "work": {
        "id": wid, "title": title, "disambiguation": "", "iswcs": [],
        "language": None, "languages": [], "type": "Song", "type-id": None,
        "attributes": [], "relations": artist_rels}})


def recording(n, title, length, ac, isrcs=(), disamb="", rels=(), video=False):
    rec = {"id": f"00000000-0000-0000-0003-0000000000{n:02d}", "title": title,
           "length": length, "video": video, "disambiguation": disamb,
           "artist-credit": ac, "aliases": [], "isrcs": list(isrcs),
           "first-release-date": "2009-01-01"}
    if rels:
        rec["relations"] = list(rels)
    return rec


def track(n, pos, number, title, length, ac, rec):
    return {"id": f"00000000-0000-0000-0004-0000000000{n:02d}", "position": pos,
            "number": number, "title": title, "length": length,
            "artist-credit": ac, "recording": rec}


def rg(n, title, ptype, stypes, date, ac, genres=(), tags=()):
    return {"id": f"99999999-0000-0000-0000-0000000001{n:02d}", "title": title,
            "primary-type": ptype, "primary-type-id": "p" if ptype else None,
            "secondary-types": list(stypes), "secondary-type-ids": ["s"] * len(stypes),
            "first-release-date": date, "disambiguation": "", "aliases": [],
            "genres": list(genres), "tags": list(tags), "artist-credit": ac}


def genre(name, count):
    return {"name": name, "count": count, "id": "g", "disambiguation": ""}


def url_rel(kind, url):
    return rel_base(**{"type": kind, "target-type": "url",
                       "url": {"id": "u", "resource": url}})


def release(rid, title, ac, group, media, **kw):
    r = {"id": rid, "title": title, "status": "Official", "status-id": "s",
         "disambiguation": "", "date": "", "country": None, "barcode": None,
         "asin": None, "quality": "normal", "packaging": None, "packaging-id": None,
         "text-representation": {"script": "Latn", "language": "eng"},
         "artist-credit": ac, "aliases": [], "genres": [], "tags": [],
         "release-group": group, "label-info": [], "release-events": [],
         "cover-art-archive": {"artwork": False, "count": 0, "front": False,
                               "back": False, "darkened": False},
         "media": media}
    r.update(kw)
    return r


def medium(pos, fmt, title, tracks):
    return {"position": pos, "format": fmt, "format-id": "f", "title": title,
            "track-count": len(tracks), "track-offset": 0, "tracks": tracks,
            "id": f"m{pos}"}


# ---------------------------------------------------------------------------
# 3: a VA live compilation on two titled discs, with works and relations.
VA_AC = credit((VA, "Various Artists", ""))
AB = credit((A, "A.", " feat. "), (B, "Artist B", ""))
AC_ = credit((A, "Artist A", " & "), (C, "Artist C", ""))
rec1 = recording(1, "Ocean Song", 200500, AB, isrcs=["GBAAA0900001", "GBAAA0900002"],
                 rels=[work_rel("wwwwwwww-0000-0000-0000-000000000001", "Ocean Song",
                                [artist_rel("composer", COMP1), artist_rel("lyricist", LYR),
                                 artist_rel("composer", COMP2)]),
                       artist_rel("arranger", ARR), artist_rel("remixer", REMIX),
                       artist_rel("producer", B)])
rec2 = recording(2, "Night Drive", 185000, credit((BAND, "The Band", "")))
rec3 = recording(3, "Ocean Song", 230000, AC_, disamb="live",
                 rels=[work_rel("wwwwwwww-0000-0000-0000-000000000001", "Ocean Song",
                                [artist_rel("composer", COMP1)])])
rec4 = recording(4, "Final Call", 199000, credit((B, "Artist B", "")))
rel3 = release(
    "33333333-3333-3333-3333-333333333333", "Sampler of Sounds", VA_AC,
    rg(3, "Sampler of Sounds", "Album", ["Compilation", "Live"], "2009-06-15", VA_AC,
       genres=[genre("rock", 3), genre("pop", 1)]),
    [medium(1, "CD", "The Early Years", [
        track(1, 1, "1", "Ocean Song", 200000, AB, rec1),
        track(2, 2, "2", "Night Drive", 185000, credit((BAND, "Band", "")), rec2)]),
     medium(2, "CD", "Live Cuts", [
        track(3, 1, "1", "Ocean Song (Live)", None, AC_, rec3),
        track(4, 2, "2", "Final Call (edit)", 199000, credit((B, "Artist B", "")), rec4)])],
    date="2010-02-03", country="US", barcode="", asin="B000TEST01",
    disambiguation="", genres=[genre("pop", 4), genre("jazz", 1)],
    tags=[{"name": "live", "count": 2}, {"name": "jazz", "count": 2}],
    **{"label-info": [{"catalog-number": "SMP 001", "label": {
        "id": "l3", "name": "Sampler Records", "sort-name": "Sampler Records",
        "aliases": [], "disambiguation": "", "label-code": None, "type": None,
        "type-id": None, "genres": [], "tags": []}}],
       "release-events": [
           {"date": "2010-02-03", "area": {"id": "us", "name": "United States",
            "sort-name": "United States", "disambiguation": "",
            "iso-3166-1-codes": ["US"], "type": None, "type-id": None}},
           {"date": "2010-03", "area": {"id": "de", "name": "Germany",
            "sort-name": "Germany", "disambiguation": "",
            "iso-3166-1-codes": ["DE"], "type": None, "type-id": None}}],
       "relations": [url_rel("discogs", "https://www.discogs.com/release/123456"),
                     url_rel("purchase for download",
                             "https://someband.bandcamp.com/album/sampler")]})

# 4: a multi-artist EP (remix), promotion, no label, digital media, a year
# and month only.
ALB = credit((A, "A.", " & "), (B, "Artist B", " vs. "), (C, "C", ""))
rel4 = release(
    "44444444-4444-4444-4444-444444444444", "Duets Collection", ALB,
    rg(4, "Duets Collection", "EP", ["Remix"], "2010-05", ALB),
    [medium(1, "Digital Media", "", [
        track(5, 1, "A1", "First Duet", 150000, ALB,
              recording(5, "First Duet", 150000, ALB)),
        track(6, 2, "A2", "Second Duet", 160000, credit((A, "Artist A", "")),
              recording(6, "Second Duet", 160000, credit((A, "Artist A", "")),
                        rels=[artist_rel("remixer", REMIX), artist_rel("remixer", ARR)])),
        track(7, 3, "B1", "Third Duet", 170000, credit((C, "C", " x "), (B, "B", "")),
              recording(7, "Third Duet", 170000, credit((C, "Artist C", " x "), (B, "Artist B", ""))))])],
    status="Promotion", date="2010-05", country="XW",
    disambiguation="promo",
    **{"label-info": [{"catalog-number": None, "label": {
        "id": "l4", "name": "[no label]", "sort-name": "[no label]", "aliases": [],
        "disambiguation": "", "label-code": None, "type": None, "type-id": None,
        "genres": [], "tags": []}}]})

for r in (rel3, rel4):
    with open(f"{OUT}/release-{r['id']}.json", "w") as f:
        json.dump(r, f, indent=1, ensure_ascii=False)
        f.write("\n")
