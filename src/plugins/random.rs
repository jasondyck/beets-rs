//! A port of beets 2.14's `random` plugin (`beetsplug/random.py`):
//! `beet random` prints a random selection of items or albums, optionally
//! balanced by a field (`-e`) or bounded by a total length (`-t`).

use crate::config::Config;
use crate::library::Library;
use crate::model::{Kind, Record};
use crate::query::Query;
use crate::template::Template;
use anyhow::Result;
use std::collections::HashMap;

/// The plugin: only a command, no import hooks.
pub struct RandomPlugin;

impl crate::plugins::Plugin for RandomPlugin {
    fn name(&self) -> &'static str {
        "random"
    }
}

/// `beet random`'s options.
#[derive(Debug, Default, Clone)]
pub struct CmdOpts {
    pub album: bool,
    pub equal_chance: bool,
    pub field: String,
    pub limit: Option<usize>,
    pub number: i64,
    pub time: Option<f64>,
    pub query: Vec<String>,
}

/// A small SplitMix64 generator: the plugin needs no reproducible stream
/// (beets uses Python's `random`), just an unseeded shuffle.
pub struct Rng(u64);

impl Rng {
    #[cfg(test)]
    pub fn from_seed(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn new() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Rng(nanos ^ (std::process::id() as u64).rotate_left(32) ^ 0x9e37_79b9_7f4a_7c15)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// `random.randrange(n)`.
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// `random.shuffle`.
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}

impl Default for Rng {
    fn default() -> Self {
        Rng::new()
    }
}

/// `_equal_chance_permutation`: every group of objects sharing a value of
/// `field` has an equal chance of coming next (objects without the field
/// are dropped).
pub fn equal_chance_permutation(objs: Vec<Record>, field: &str, rng: &mut Rng) -> Vec<Record> {
    let mut groups: Vec<(String, Vec<Record>)> = vec![];
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut keyed: Vec<(String, Record)> = objs
        .into_iter()
        .filter_map(|o| {
            let v = o.get(field)?;
            if v == crate::model::Value::Null {
                None
            } else {
                Some((v.raw(), o))
            }
        })
        .collect();
    // Python sorts by the field's values (strings here).
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    for (k, o) in keyed {
        match index.get(&k) {
            Some(&i) => groups[i].1.push(o),
            None => {
                index.insert(k.clone(), groups.len());
                groups.push((k, vec![o]));
            }
        }
    }
    for (_, vals) in groups.iter_mut() {
        rng.shuffle(vals);
    }
    let mut out = vec![];
    while !groups.is_empty() {
        let i = rng.below(groups.len());
        let o = groups[i].1.pop().unwrap();
        out.push(o);
        if groups[i].1.is_empty() {
            groups.remove(i);
            // The removed group's key no longer maps; rebuild the index.
            index.clear();
            for (n, (k, _)) in groups.iter().enumerate() {
                index.insert(k.clone(), n);
            }
        }
    }
    out
}

/// `_take_time`: yield objects without exceeding `secs` seconds.
fn take_time(objs: Vec<Record>, secs: f64) -> Vec<Record> {
    let mut total = 0.0;
    let mut out = vec![];
    for o in objs {
        let length = o.float("length");
        if total + length <= secs {
            out.push(o);
            total += length;
        }
    }
    out
}

/// `random_objs`.
pub fn random_objs(
    objs: Vec<Record>,
    field: &str,
    number: i64,
    time_minutes: Option<f64>,
    equal_chance: bool,
    rng: &mut Rng,
) -> Vec<Record> {
    let perm = if equal_chance {
        equal_chance_permutation(objs, field, rng)
    } else {
        let mut perm = objs;
        rng.shuffle(&mut perm);
        perm
    };
    if let Some(t) = time_minutes.filter(|t| *t != 0.0) {
        return take_time(perm, t * 60.0);
    }
    perm.into_iter().take(number.max(0) as usize).collect()
}

/// `beet random`.
pub fn command(cfg: &Config, lib: &Library, opts: CmdOpts) -> Result<()> {
    run(cfg, lib, opts, &mut std::io::stdout().lock())
}

/// `random_func`, writing to `out`.
pub fn run(
    cfg: &Config,
    lib: &Library,
    opts: CmdOpts,
    out: &mut impl std::io::Write,
) -> Result<()> {
    let kind = if opts.album { Kind::Album } else { Kind::Item };
    let q = Query::parse(kind, &opts.query)?;
    let mut objs = if opts.album {
        lib.albums(&q)?
    } else {
        lib.items(&q)?
    };
    objs.truncate(opts.limit.unwrap_or(usize::MAX));
    let chosen = random_objs(
        objs,
        &opts.field,
        opts.number,
        opts.time,
        opts.equal_chance,
        &mut Rng::new(),
    );
    let fmt = if opts.album {
        cfg.str("format_album")
    } else {
        cfg.str("format_item")
    };
    let t = Template::parse(&fmt);
    for o in chosen {
        writeln!(out, "{}", crate::commands::format_record(&t, &o, Some(lib)))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "random_tests.rs"]
mod tests;
