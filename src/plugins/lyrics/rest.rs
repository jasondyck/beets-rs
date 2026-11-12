//! beets' `RestFiles`: `lyrics -r DIR` writes the lyrics as a Sphinx
//! (reStructuredText) project, one file per album artist.

use super::slug;
use crate::model::Item;
use anyhow::Result;
use std::path::{Path, PathBuf};

const REST_INDEX_TEMPLATE: &str = "Lyrics
======

* :ref:`Song index <genindex>`
* :ref:`search`

Artist index:

.. toctree::
   :maxdepth: 1
   :glob:

   artists/*";

const REST_CONF_TEMPLATE: &str = r#"master_doc = "index"
project = "Lyrics"
copyright = "none"
author = "Various Authors"
latex_documents = [
    (master_doc, "Lyrics.tex", project, author, "manual"),
]
epub_exclude_files = ["search.html"]
epub_tocdepth = 1
epub_tocdup = False"#;

/// `textwrap.indent(text, prefix)`: every line with non-space content.
fn indent(text: &str, prefix: &str) -> String {
    text.split_inclusive('\n')
        .map(|l| {
            if l.trim().is_empty() {
                l.to_string()
            } else {
                format!("{prefix}{l}")
            }
        })
        .collect()
}

fn underline(text: &str, c: char) -> String {
    format!("{text}\n{}", c.to_string().repeat(text.chars().count()))
}

pub struct RestFiles {
    pub directory: PathBuf,
}

impl RestFiles {
    /// conf.py and index.rst for Sphinx, unless they exist (so that
    /// customizations are respected).
    fn write_indexes(&self) -> Result<()> {
        let index = self.directory.join("index.rst");
        if !index.exists() {
            std::fs::write(index, REST_INDEX_TEMPLATE)?;
        }
        let conf = self.directory.join("conf.py");
        if !conf.exists() {
            std::fs::write(conf, REST_CONF_TEMPLATE)?;
        }
        Ok(())
    }

    fn write_artist(&self, artist: &str, items: &[&Item]) -> Result<()> {
        let mut parts = vec![
            underline(artist, '='),
            ".. contents::\n   :local:".to_string(),
        ];
        // Consecutive items of the same album (`itertools.groupby`).
        let mut prev_album: Option<&str> = None;
        for item in items {
            let album = item.str("album");
            if prev_album != Some(album) {
                parts.push(underline(album, '-'));
                prev_album = Some(album);
            }
            let title = format!(":index:`{}`", item.str("title").trim());
            parts.push(underline(&title, '~'));
            parts.push(indent(item.str("lyrics"), "| "));
        }
        let dir = self.directory.join("artists");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join(format!("{}.rst", slug(artist))),
            parts.join("\n\n").trim(),
        )?;
        Ok(())
    }

    pub fn write(&self, items: &[Item]) -> Result<()> {
        std::fs::create_dir_all(&self.directory)?;
        self.write_indexes()?;
        let mut items: Vec<&Item> = items.iter().collect();
        items.sort_by(|a, b| a.str("albumartist").cmp(b.str("albumartist")));
        for group in items.chunk_by(|a, b| a.str("albumartist") == b.str("albumartist")) {
            self.write_artist(group[0].str("albumartist").trim(), group)?;
        }
        let d = self.directory.display();
        let sub = |s: &str| Path::new(&self.directory).join(s).display().to_string();
        println!(
            "\nReST files generated. to build, use one of:\n  sphinx-build -b html  {d} {}\n  sphinx-build -b epub  {d} {}\n  sphinx-build -b latex {d} {} && make -C {} all-pdf\n",
            sub("html"),
            sub("epub"),
            sub("latex"),
            sub("latex"),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // beets' TestRestFiles.
    #[test]
    fn writes_files() {
        let d = tempfile::tempdir().unwrap();
        let items: Vec<Item> = [
            ("Artist One", "Album One", "Song One", "Lyrics One"),
            ("Artist One", "Album One", "Song Two", "Lyrics Two"),
            ("Artist Two", "Album Two", "Song Three", "Lyrics Three"),
        ]
        .iter()
        .map(|(aa, a, t, l)| {
            let mut i = Item::new_item();
            i.set_str("albumartist", *aa);
            i.set_str("album", *a);
            i.set_str("title", *t);
            i.set_str("lyrics", *l);
            i
        })
        .collect();
        RestFiles {
            directory: d.path().to_path_buf(),
        }
        .write(&items)
        .unwrap();
        assert!(d.path().join("index.rst").exists());
        assert!(d.path().join("conf.py").exists());
        let one = std::fs::read_to_string(d.path().join("artists/artist-one.rst")).unwrap();
        assert_eq!(
            one,
            "Artist One\n==========\n\n.. contents::\n   :local:\n\nAlbum One\n---------\n\n\
             :index:`Song One`\n~~~~~~~~~~~~~~~~~\n\n| Lyrics One\n\n\
             :index:`Song Two`\n~~~~~~~~~~~~~~~~~\n\n| Lyrics Two"
        );
        let two = std::fs::read_to_string(d.path().join("artists/artist-two.rst")).unwrap();
        let pos = |s: &str| two.find(s).unwrap();
        assert!(pos("Artist Two") < pos("Album Two"));
        assert!(pos("Song Three") < pos("Lyrics Three"));
    }

    #[test]
    fn indent_skips_blank_lines() {
        assert_eq!(indent("a\n\n b\n", "| "), "| a\n\n|  b\n");
    }
}
