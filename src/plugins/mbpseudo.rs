//! A port of beets 2.14's `mbpseudo` plugin (`beetsplug/mbpseudo.py`):
//! offer translated/transliterated pseudo-releases as MusicBrainz
//! candidates. Configure `scripts` (e.g. `[Latn]`) and it replaces the
//! `musicbrainz` plugin as the autotagger's source (keeping both enabled is
//! an error, as in beets); with `custom_tags_only`, only the custom tags
//! are filled from the pseudo-release.
//!
//! The work is in `mb.rs` (`wanted_pseudo_release`, `add_custom_tags` and
//! `MusicBrainz::album_for_id_variants`), driven by `ParseOpts`.

use crate::config::Config;
use crate::plugins::Plugin;

pub struct MBPseudoPlugin;

impl Plugin for MBPseudoPlugin {
    fn name(&self) -> &'static str {
        "mbpseudo"
    }
}

pub fn load(cfg: &Config) -> anyhow::Result<MBPseudoPlugin> {
    if crate::plugins::enabled(cfg, "musicbrainz") {
        anyhow::bail!(
            "The musicbrainz plugin should not be enabled together with the mbpseudo plugin"
        );
    }
    Ok(MBPseudoPlugin)
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "mbpseudo",
        order: 69,
        defaults: r#"mbpseudo:
    scripts: []
    custom_tags_only: false
    album_custom_tags:
        album_transl: album
        album_artist_transl: artist
    track_custom_tags:
        title_transl: title
        artist_transl: artist
"#,
        config_keys: &["mbpseudo"],
        redacted: &[],
        make: Some(registry_make),
        fatal: true,
        commands: &[],
    }
}

fn registry_make(
    cfg: &crate::config::Config,
) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)?))
}

#[cfg(test)]
#[path = "mbpseudo_tests.rs"]
mod tests;
