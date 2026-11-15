//! Micro-benchmarks for beets-rs' hot paths: what `ls`, `import` and the
//! autotagger spend their time in. Run with `cargo bench --bench core`;
//! a single group with e.g. `cargo bench --bench core -- template`.
//!
//! The library benches use an in-memory database filled with the same
//! shape of synthetic library as `tools/bench/bench.sh` (10 tracks per
//! album, 50 artists, 6 genres, 60 years).

use std::hint::black_box;
use std::path::PathBuf;

use beets_rs::autotag::{Ctx, string_dist, track_distance};
use beets_rs::config::Config;
use beets_rs::formatted::Formatted;
use beets_rs::library::Library;
use beets_rs::mb::TrackInfo;
use beets_rs::model::{Item, Kind, Record};
use beets_rs::paths::PathFormatter;
use beets_rs::query::{Obj, Query};
use beets_rs::template::Template;
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

const GENRES: [&str; 6] = ["Rock", "Jazz", "Pop", "Electronic", "Folk", "Classical"];

/// Item `i` of the synthetic library (album `i / 10`, track `i % 10`).
fn synthetic_item(i: usize) -> Item {
    let (a, t) = (i / 10, i % 10);
    let artist = format!("Artist {}", a % 50);
    let mut item = Record::new_item();
    item.set_str("artist", artist.clone());
    item.set_str("albumartist", artist);
    item.set_str("album", format!("Album {a}"));
    item.set_str("title", format!("Song {i} ({})", t + 1));
    item.set_int("track", i64::try_from(t + 1).expect("small"));
    item.set_int("tracktotal", 10);
    item.set_int("year", i64::try_from(1960 + a % 60).expect("small"));
    item.set_str("genre", GENRES[a % 6]);
    item.set_str("format", "MP3");
    item.set_int("bitrate", 32_000);
    item.set_path(PathBuf::from(format!("/music/{a:05}/{t:02}.mp3")));
    item
}

/// A fresh on-disk library in a temporary directory (kept alive by the
/// returned guard).
fn temp_library() -> (tempfile::TempDir, Library) {
    let dir = tempfile::tempdir().expect("temp dir");
    let lib = Library::open_with(&dir.path().join("library.db"), None, false).expect("library");
    (dir, lib)
}

/// An on-disk library of `n` items in albums of 10.
fn synthetic_library(n: usize) -> (tempfile::TempDir, Library) {
    let (dir, lib) = temp_library();
    lib.transaction(|lib| {
        for a in 0..n.div_ceil(10) {
            let mut items: Vec<Item> = (a * 10..((a + 1) * 10).min(n))
                .map(|i| {
                    let mut item = synthetic_item(i);
                    lib.add(&mut item).expect("add item");
                    item
                })
                .collect();
            lib.add_album(&mut items).expect("add album");
        }
        Ok(())
    })
    .expect("fill library");
    (dir, lib)
}

fn bench_template(c: &mut Criterion) {
    let mut g = c.benchmark_group("template");
    let default_path = "$albumartist/$album%aunique{}/$track $title";
    let fancy = "%if{$year,[$year] }%upper{%left{$artist,3}} - $title%ifdef{genre, ($genre)}";
    g.bench_function("parse/default_path", |b| {
        b.iter(|| Template::parse(black_box(default_path)));
    });
    g.bench_function("parse/functions", |b| {
        b.iter(|| Template::parse(black_box(fancy)));
    });

    let item = synthetic_item(1234);
    let ls_fmt = Template::parse("$artist - $album - $title");
    let fancy = Template::parse(fancy);
    g.bench_function("render/ls_default", |b| {
        b.iter(|| ls_fmt.render(&Formatted::new(black_box(&item), None)));
    });
    g.bench_function("render/functions", |b| {
        b.iter(|| fancy.render(&Formatted::new(black_box(&item), None)));
    });
    g.finish();
}

fn bench_query(c: &mut Criterion) {
    let mut g = c.benchmark_group("query");
    let cases: &[(&str, &[&str])] = &[
        ("field", &["artist:Artist 7"]),
        ("regex", &["title::^Song 1"]),
        ("range", &["year:1970..1979"]),
        ("multi_sort", &["genre:Rock", "year-", "title+"]),
        ("any_field", &["song"]),
    ];
    for (name, args) in cases {
        g.bench_with_input(BenchmarkId::new("parse", name), args, |b, args| {
            b.iter(|| Query::parse(Kind::Item, black_box(args)).expect("query"));
        });
    }

    // In-Rust matching over a batch of items (the slow-query path).
    let items: Vec<Item> = (0..1000).map(synthetic_item).collect();
    g.throughput(Throughput::Elements(items.len() as u64));
    for (name, args) in cases {
        let q = Query::parse(Kind::Item, args).expect("query");
        g.bench_with_input(BenchmarkId::new("match_1k", name), &q, |b, q| {
            b.iter(|| items.iter().filter(|i| q.matches_obj(&Obj::new(i))).count());
        });
    }
    g.finish();
}

fn bench_library(c: &mut Criterion) {
    const N: usize = 10_000;
    let mut g = c.benchmark_group("library");
    g.sample_size(20);
    let (_dir, lib) = synthetic_library(N);

    let cases: &[(&str, &[&str])] = &[
        ("all_items", &[]),
        ("field", &["artist:Artist 7"]),
        ("regex", &["title::^Song 1"]),
        ("range", &["year:1970..1979"]),
        ("sorted", &["year-", "title+"]),
    ];
    for (name, args) in cases {
        let q = Query::parse(Kind::Item, args).expect("query");
        g.bench_with_input(BenchmarkId::new("items_10k", name), &q, |b, q| {
            b.iter(|| lib.items(q).expect("items").len());
        });
    }
    let all_albums = Query::parse::<&str>(Kind::Album, &[]).expect("query");
    g.bench_function("albums_1k/all", |b| {
        b.iter(|| lib.albums(&all_albums).expect("albums").len());
    });

    // `ls`: query plus formatting every item.
    let all = Query::parse::<&str>(Kind::Item, &[]).expect("query");
    let fmt = Template::parse("$artist - $album - $title");
    g.bench_function("ls_10k/default_format", |b| {
        b.iter(|| {
            let items = lib.items(&all).expect("items");
            items
                .iter()
                .map(|i| fmt.render(&Formatted::new(i, Some(&lib))).len())
                .sum::<usize>()
        });
    });

    g.throughput(Throughput::Elements(100));
    g.bench_function("add_100_items", |b| {
        b.iter_batched(
            || {
                (
                    temp_library(),
                    (0..100).map(synthetic_item).collect::<Vec<_>>(),
                )
            },
            |((_dir, lib), mut items)| {
                lib.transaction(|lib| {
                    for item in &mut items {
                        lib.add(item)?;
                    }
                    Ok(())
                })
                .expect("add");
            },
            BatchSize::SmallInput,
        );
    });

    // Attribute-loading changes must help populated plugin libraries too.
    lib.transaction(|lib| {
        for mut item in lib.items(&all)? {
            for (key, value) in [("mood", "calm"), ("source", "cd"), ("rating", "5")] {
                item.flex.insert(key.into(), value.into());
            }
            lib.store(&item)?;
        }
        Ok(())
    })
    .expect("populate flex attributes");
    g.throughput(Throughput::Elements(N as u64));
    for (name, args) in [("all", &[][..]), ("field", &["artist:Artist 7"][..])] {
        let q = Query::parse(Kind::Item, args).expect("query");
        g.bench_with_input(BenchmarkId::new("items_flex_10k", name), &q, |b, q| {
            b.iter(|| lib.items(q).expect("items").len());
        });
    }
    g.finish();
}

fn bench_paths(c: &mut Criterion) {
    let mut g = c.benchmark_group("paths");
    let cfg = Config::defaults();
    let formatter = PathFormatter::new(&cfg).expect("path formatter");
    let item = synthetic_item(1234);
    g.bench_function("destination/default", |b| {
        b.iter(|| formatter.destination(black_box(&item), None, None));
    });
    g.finish();
}

fn bench_library_open(c: &mut Criterion) {
    let (dir, lib) = synthetic_library(10_000);
    let path = dir.path().join("library.db");
    // No connection remains open: each iteration pays the same connection
    // setup and teardown as a separate CLI invocation on an existing library.
    drop(lib);
    let mut g = c.benchmark_group("library");
    g.sample_size(20);
    g.bench_function("open_10k", |b| {
        b.iter(|| Library::open_with(black_box(&path), None, false).expect("library"));
    });
    let cases: &[(&str, &[&str])] = &[
        ("field", &["artist:Artist 7"]),
        ("regex", &["title::^Song 1"]),
        ("all", &[]),
    ];
    for (name, args) in cases {
        let q = Query::parse(Kind::Item, args).expect("query");
        g.bench_with_input(BenchmarkId::new("open_and_query_10k", name), &q, |b, q| {
            b.iter(|| {
                let lib = Library::open_with(&path, None, false).expect("library");
                lib.items(q).expect("items").len()
            });
        });
    }
    g.finish();
}

fn bench_autotag(c: &mut Criterion) {
    let mut g = c.benchmark_group("autotag");
    let pairs = [
        ("short_equal", "Song 1", "Song 1"),
        ("short_diff", "Song 1", "Sng One"),
        ("featuring", "Something (feat. Someone Else)", "Something"),
        ("the_suffix", "The Beatles", "Beatles, The"),
        (
            "long",
            "A Fairly Long Track Title With Many Words (Live at the Venue, 1977)",
            "A Fairly Long Track Title with Many Words - Live at The Venue 1977",
        ),
    ];
    for (name, a, b2) in pairs {
        g.bench_function(BenchmarkId::new("string_dist", name), |b| {
            b.iter(|| string_dist(black_box(Some(a)), black_box(Some(b2))));
        });
    }

    let cfg = Config::defaults();
    let ctx = Ctx::new(&cfg);
    let item = synthetic_item(1234);
    let track = TrackInfo {
        title: "Song 1234 (5)".into(),
        artist: "Artist 23".into(),
        length: Some(1.0),
        index: 5,
        medium_index: 5,
        ..TrackInfo::default()
    };
    g.bench_function("track_distance", |b| {
        b.iter(|| track_distance(&ctx, black_box(&item), black_box(&track), true).distance());
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_template,
    bench_query,
    bench_library,
    bench_library_open,
    bench_paths,
    bench_autotag
);
criterion_main!(benches);
