"""Writes cases.json: what CPython gives for inline-style expressions and
function bodies (with the variables below), which src/inline is tested
against. Run with the reference venv's Python:
    ~/.cache/beets-reference/venv/bin/python -I tests/fixtures/inline/gen.py
"""
import json
import os

GLOBALS = {
    "title": "Hello World", "artist": "The Beatles", "albumartist": "", "album": "Abbey Road",
    "track": 3, "tracktotal": 12, "disc": 1, "disctotal": 2, "year": 1969, "month": 0,
    "length": 187.5, "bitrate": 320000, "comp": 0, "genres": ["Rock", "Pop"],
    "path": b"/music/The Beatles/01 Come.mp3", "rg_track_gain": None, "format": "MP3",
    "label": "Apple", "id": 7, "album_id": 2, "items": [1, 2, 3],
}

EXPRS = r"""
1//0
1%0
1/0
1.0//0
'a'+1
1+'a'
'a'<1
int('x')
'abc'[5]
[1][3]
{}['k']
None.x
's'.foo
len(5)
x
round(2.5)
round(2.675,2)
'%5.2f|%-4d|%x|%r' % (3.14159, 7, 255, 'q')
f'{3.0:g}|{1e20:g}|{0.0001:g}|{12345678.9:g}|{1234:,}|{0.25:%}|{5:+05d}|{-3:>6}|{"ab":^7}|{255:#x}'
str(1e16)+str(1e-5)+str(0.1+0.2)
'a'*3
[1]*2
3*'x'
divmod(-7,2)
-7//2
-7%2
2**-1
abs(-0.0)
min('b','a')
max([3,1])
sorted('cba')
'a b  c'.split()
'x'.join(['a','b'])
'%s' % None
'%d' % 3.7
f'{None}'
str(True+1)
bool('')
'abc'.title()
'hello world'.capitalize()
'  x '.strip()
'a,b'.split(',',1)
'abc'.startswith(('x','a'))
[1,'a',None]
(1,)
{'a':1}
b'x'
'%(a)s' % {'a':1}
int(3.9)
int(' 12 ')
float('1e3')
str(1.0)
1 < 'a'
[1,2] < [1,3]
'x' in 'xyz'
3 in [1,2]
None or 'd'
0 and 1
'{0}-{1:02d}-{x}'.format('a', 3, x=1)
'{:>5}'.format('ab')
'abc'[::-1]
'abc'[-1]
'abc'[1:]
len(b'abc')
b'abc'.decode()
'ÄB'.lower()
1 if 0 else 2
round(-0.5)
round(1.5)
round(3.14159, 2)
f'{2.5:.0f}'
f'{0.125:.2f}'
'%.0f' % 0.5
'%.1f' % 0.25
f'{1234567:_}'
f'{12:b}'
f'{12:o}'
f'{65:c}'
f'{3:e}'
f'{123.456:.2e}'
f'{1.5:G}'
str(float('inf'))
f'{"x"!r:>5}'
'%c' % 65
'%5s|%-5s|' % ('a','b')
'%+d % d' % (5, 5)
'%03d' % -5
'%e' % 12345.678
'%g' % 0.00001234
'%#o' % 8
albumartist[0].upper() + u'.'
artist[0].upper() + u'.'
f"{disc:02d}.{track:02d}" if disctotal > 1 else f"{track:02d}"
f'{disc:02d}-{track:02d}' if disctotal > 1 else f'{track:02d}'
'%02d' % track
str(track).zfill(2)
artist.lower().replace(' ', '_')
artist[4:] + ', The' if artist.startswith('The ') else artist
title.split()[0]
' '.join(w.capitalize() for w in title.split())
len(genres)
'; '.join(genres)
genres[0] if genres else 'Unknown'
path.decode('utf-8').split('/')[-1]
path.decode().rsplit('/', 1)[0]
int(length // 60)
'%d:%02d' % divmod(int(length), 60)
f'{int(length) // 60}:{int(length) % 60:02d}'
round(bitrate / 1000)
f'{bitrate // 1000}kbps'
year or 'Unknown'
str(year)[:3] + '0s' if year else 'Unknown'
f'{year // 10 * 10}s'
'Compilations' if comp else albumartist or artist
rg_track_gain is None
rg_track_gain or 0.0
format.lower()
title[:20]
title.upper() == title
not comp
artist if len(artist) < 5 else artist[:5]
[g.upper() for g in genres if g != 'Pop']
sum(i for i in items)
sum(items) / len(items)
max(items) - min(items)
any(g == 'Rock' for g in genres)
title.find('World')
title.count('l')
title.center(15, '*')
title.ljust(14, '.')
title.rjust(14)
title.partition(' ')
title.swapcase()
'x'.join(str(i) for i in range(3))
list(range(10, 0, -3))
list(enumerate('ab', 1))
list(zip('ab', [1, 2]))
sorted(genres, reverse=True)
''.join(reversed(title))
'{} - {}'.format(artist, title)
'{0[0]}|{1}'.format(genres, len(genres))
isinstance(track, int)
isinstance(title, (int, str))
chr(ord('a') + 1)
hex(255)
f'{track!s:>4}|{title!r}'
f"{'nested ' + f'{track}'}"
(track, disc) > (2, 9)
track if track else None
-track
~track
track << 2
2 ** 10
7 / 2
-7 / 2
7.5 // 2
-7.5 % 2
title * 0
'abc' < 'abd'
[] or [0]
'a' 'b' "c"
r'\d+'
'tab\there'
'é\x41'
3 < track < 5
track == 3.0
True + True
1e300 * 1e10
int('0x1f', 16)
float('nan') == float('nan')
title.split(' ', 0)
'  a  b '.split(None, 1)
'a  b  '.rsplit(None, 1)
'a-b-c'.rsplit('-', 1)
title.removeprefix('Hello ')
"it's"
'say "hi"'
'both \' and "'
title.encode()
1_000
0o17 + 0b11
.5 + 5.
title.lower.x
""".strip().split("\n")

BODIES = [
    "x = track * 2\nif x > 5:\n    return 'big'\nreturn 'small'",
    "return track + 1",
    "total = 0\nfor i in items:\n    total += i\nreturn total / len(items)",
    "if disctotal > 1:\n    return '%02d-%02d' % (disc, track)\nelif track:\n    return '%02d' % track\nelse:\n    return ''",
    "out = []\nfor g in genres:\n    if g == 'Pop':\n        continue\n    out.append(g.lower())\nreturn ', '.join(out)",
    "n = 0\nwhile n < 5:\n    n += 2\nreturn n",
    "a, b = divmod(int(length), 60)\nreturn f'{a}:{b:02d}'",
    "track = track + 1\nreturn track",
    "x = 1\nx = x + 1\nreturn x",
    "for i in items:\n    if i == 2:\n        break\nelse:\n    return 'none'\nreturn i",
    "d = {'Rock': 'R'}\nreturn d.get(genres[0], '?') + d.get('x', '?')",
    "s = title\ns += '!'\nreturn s",
    "l = [3, 1, 2]\nl.sort()\nl.reverse()\nreturn l",
    "if not genres: return None\nreturn genres[-1]",
    "pass",
    "x = [1, 2]\nx[0] = 9\nreturn x",
    "return",
    "a = b = 3\nreturn a + b",
    "first, *rest = genres\nreturn first",
]

out_exprs = []
for e in EXPRS:
    g = dict(GLOBALS)
    try:
        out_exprs.append([e, {"repr": repr(eval("(" + e + ")", g))}])
    except Exception as ex:
        out_exprs.append([e, {"error": [type(ex).__name__, str(ex)]}])

out_bodies = []
for b in BODIES:
    g = dict(GLOBALS)
    try:
        src = "def f(db_obj):\n    " + b.replace("\n", "\n    ")
        env = {}
        eval(compile(src, "inline", "exec"), env)
        f = env["f"]
        f.__globals__.update(g)
        out_bodies.append([b, {"repr": repr(f(None))}])
    except SyntaxError as ex:
        out_bodies.append([b, {"syntax": str(ex)}])
    except Exception as ex:
        out_bodies.append([b, {"error": [type(ex).__name__, str(ex)]}])

with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "cases.json"), "w") as f:
    json.dump({"globals": {k: (v.decode() if isinstance(v, bytes) else v)
                           for k, v in GLOBALS.items()},
               "exprs": out_exprs, "bodies": out_bodies}, f, ensure_ascii=False, indent=0)
