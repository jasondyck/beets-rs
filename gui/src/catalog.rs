//! The plugin toolbox's catalog: every plugin bundled with beets-rs (from
//! its registry), with a short description and a category for browsing,
//! its commands and its default settings.

use beets_rs::plugin_registry;

/// The categories, in the order the toolbox lists them.
pub const CATEGORIES: &[&str] = &[
    "Album art",
    "Tags & metadata",
    "Lyrics",
    "Audio analysis",
    "Metadata sources",
    "Import",
    "Files & paths",
    "Library tools",
    "Playback & playlists",
    "Media servers",
    "Online services",
];

/// (plugin, category, what it does), in plain words. A plugin missing here
/// still appears, under "Other", with no description.
const INFO: &[(&str, &str, &str)] = &[
    // Album art
    (
        "fetchart",
        "Album art",
        "Find and download cover art for albums: from the album's folder, the Cover Art Archive, iTunes, Amazon, Wikipedia, fanart.tv and more. Runs on import, or on demand.",
    ),
    (
        "embedart",
        "Album art",
        "Embed album covers into the music files themselves, so every player shows them. Also extracts or clears embedded art.",
    ),
    (
        "thumbnails",
        "Album art",
        "Create freedesktop thumbnails of album covers, so file managers show the art on album folders.",
    ),
    // Tags & metadata
    (
        "lastgenre",
        "Tags & metadata",
        "Set genres from Last.fm's tags, filtered through a curated genre list and tree.",
    ),
    (
        "ftintitle",
        "Tags & metadata",
        "Move \"feat. X\" from the artist field into the track title.",
    ),
    (
        "titlecase",
        "Tags & metadata",
        "Capitalize titles, artists and albums in title case, with exceptions you choose.",
    ),
    (
        "zero",
        "Tags & metadata",
        "Blank out fields you don't want (comments, images, …), in the library and the files.",
    ),
    (
        "scrub",
        "Tags & metadata",
        "Strip every tag from the files and write back only beets' clean set.",
    ),
    (
        "the",
        "Tags & metadata",
        "Move articles like \"The\" or \"A\" to the end of names in paths (\"Beatles, The\").",
    ),
    (
        "rewrite",
        "Tags & metadata",
        "Rewrite field values with regular expressions for paths (for example, normalise artist spellings).",
    ),
    (
        "advancedrewrite",
        "Tags & metadata",
        "Rewrite fields depending on queries: change one field only for matching items.",
    ),
    (
        "substitute",
        "Tags & metadata",
        "Substitute text in path templates using regular-expression rules.",
    ),
    (
        "albumtypes",
        "Tags & metadata",
        "Format an album's types (album, EP, live, compilation …) for use in paths.",
    ),
    (
        "parentwork",
        "Tags & metadata",
        "Fetch the parent work and its composer from MusicBrainz, for classical music.",
    ),
    (
        "mbsync",
        "Tags & metadata",
        "Refresh metadata for already-imported music from MusicBrainz.",
    ),
    (
        "bpsync",
        "Tags & metadata",
        "Refresh metadata for Beatport-matched music from Beatport.",
    ),
    (
        "inline",
        "Tags & metadata",
        "Define computed fields with short Python-style expressions in the config.",
    ),
    (
        "types",
        "Tags & metadata",
        "Declare types (number, date, boolean …) for your own flexible fields, so they sort and query properly.",
    ),
    (
        "fromfilename",
        "Tags & metadata",
        "Guess track titles and numbers from file names when tags are missing.",
    ),
    (
        "importadded",
        "Tags & metadata",
        "Keep the files' original modification times as the date they were added.",
    ),
    // Lyrics
    (
        "lyrics",
        "Lyrics",
        "Fetch lyrics (also synced lyrics) from LRCLib, Genius and other sites, with optional translation.",
    ),
    // Audio analysis
    (
        "replaygain",
        "Audio analysis",
        "Measure loudness and write ReplayGain tags, so tracks play at an even volume.",
    ),
    (
        "autobpm",
        "Audio analysis",
        "Detect each track's tempo (BPM) from the audio.",
    ),
    (
        "bpm",
        "Audio analysis",
        "Measure a track's tempo by tapping Enter along with the beat.",
    ),
    (
        "keyfinder",
        "Audio analysis",
        "Detect the musical key with the KeyFinder tool and store it in initial_key.",
    ),
    (
        "acousticbrainz",
        "Audio analysis",
        "Fetch mood, danceability, key and rhythm data from AcousticBrainz (an archived service).",
    ),
    (
        "absubmit",
        "Audio analysis",
        "Analyse tracks and submit the data to AcousticBrainz (deprecated: the service is closed).",
    ),
    // Metadata sources
    (
        "musicbrainz",
        "Metadata sources",
        "Match albums and tracks against MusicBrainz when importing. On by default.",
    ),
    (
        "discogs",
        "Metadata sources",
        "Also search Discogs for matches when importing.",
    ),
    (
        "spotify",
        "Metadata sources",
        "Also search Spotify when importing; sync popularity and audio features from Spotify.",
    ),
    (
        "deezer",
        "Metadata sources",
        "Also search Deezer when importing; update rankings from Deezer.",
    ),
    (
        "beatport",
        "Metadata sources",
        "Also search Beatport (electronic music) when importing.",
    ),
    (
        "tidal",
        "Metadata sources",
        "Also search Tidal when importing; sync metadata from Tidal.",
    ),
    (
        "chroma",
        "Metadata sources",
        "Identify tracks by their acoustic fingerprint (AcoustID), even with no tags at all.",
    ),
    (
        "mbpseudo",
        "Metadata sources",
        "Prefer MusicBrainz pseudo-releases, such as transliterated track listings.",
    ),
    // Import
    (
        "importfeeds",
        "Import",
        "Write a playlist (or symlinks) of everything you import.",
    ),
    (
        "importsource",
        "Import",
        "Remember where imported files came from, and optionally delete the source when you remove them.",
    ),
    (
        "ihate",
        "Import",
        "Warn about or skip music you don't want, by query, when importing.",
    ),
    (
        "filefilter",
        "Import",
        "Only import files whose names match a pattern.",
    ),
    (
        "hook",
        "Import",
        "Run your own shell commands when things happen (an import, a write, …).",
    ),
    (
        "edit",
        "Import",
        "Edit metadata in your text editor, from the command line or while importing.",
    ),
    // Files & paths
    (
        "convert",
        "Files & paths",
        "Transcode music to another format (MP3, Opus, AAC …) with ffmpeg, for a phone or player.",
    ),
    (
        "bucket",
        "Files & paths",
        "Group folders into buckets, like A-F or by decade, in your path layout.",
    ),
    (
        "permissions",
        "Files & paths",
        "Set file and folder permissions on everything imported.",
    ),
    (
        "badfiles",
        "Files & paths",
        "Check files for corruption with external checkers (flac, mp3val …).",
    ),
    (
        "replace",
        "Files & paths",
        "Replace a track's audio file with another file, keeping its metadata.",
    ),
    (
        "unimported",
        "Files & paths",
        "List files in your music folder that aren't in the library.",
    ),
    // Library tools
    (
        "duplicates",
        "Library tools",
        "Find duplicate albums or tracks, and optionally merge or delete them.",
    ),
    (
        "missing",
        "Library tools",
        "List tracks missing from your albums, or albums missing for an artist.",
    ),
    (
        "info",
        "Library tools",
        "Show every tag in a file, including ones beets doesn't use.",
    ),
    (
        "export",
        "Library tools",
        "Export library data as JSON, CSV or XML.",
    ),
    (
        "limit",
        "Library tools",
        "List only the first or last N results of a query (lslimit, and a <N query).",
    ),
    ("random", "Library tools", "Pick random albums or tracks."),
    (
        "fuzzy",
        "Library tools",
        "Fuzzy-match queries with the ~ prefix, forgiving typos.",
    ),
    (
        "bareasc",
        "Library tools",
        "Match accented text with plain ASCII queries using the # prefix.",
    ),
    (
        "loadext",
        "Library tools",
        "Load SQLite extensions (for example, ICU for better sorting).",
    ),
    (
        "fish",
        "Library tools",
        "Generate tab-completion for the fish shell.",
    ),
    (
        "freedesktop",
        "Library tools",
        "(Retired upstream; use thumbnails.) Kept for compatibility.",
    ),
    // Playback & playlists
    (
        "play",
        "Playback & playlists",
        "Play the query's results in your music player.",
    ),
    (
        "playlist",
        "Playback & playlists",
        "Use your M3U playlists as queries (playlist:name).",
    ),
    (
        "smartplaylist",
        "Playback & playlists",
        "Keep M3U playlists generated from queries up to date.",
    ),
    (
        "bpd",
        "Playback & playlists",
        "A small MPD-compatible music server over your library.",
    ),
    (
        "mpdstats",
        "Playback & playlists",
        "Record play counts, skips and ratings from MPD.",
    ),
    (
        "lastimport",
        "Playback & playlists",
        "Import your play counts from Last.fm.",
    ),
    (
        "listenbrainz",
        "Playback & playlists",
        "Import listens and playlists from ListenBrainz.",
    ),
    (
        "subsonicplaylist",
        "Playback & playlists",
        "Turn playlists from a Subsonic server into flags in your library.",
    ),
    (
        "metasync",
        "Playback & playlists",
        "Import ratings and play counts from Amarok or iTunes.",
    ),
    // Media servers
    (
        "mpdupdate",
        "Media servers",
        "Tell MPD to rescan when the library changes.",
    ),
    (
        "plexupdate",
        "Media servers",
        "Tell Plex to rescan when the library changes.",
    ),
    (
        "kodiupdate",
        "Media servers",
        "Tell Kodi to rescan when the library changes.",
    ),
    (
        "embyupdate",
        "Media servers",
        "Tell Emby/Jellyfin to rescan when the library changes.",
    ),
    (
        "sonosupdate",
        "Media servers",
        "Tell Sonos to rescan when the library changes.",
    ),
    (
        "subsonicupdate",
        "Media servers",
        "Tell a Subsonic server to rescan when the library changes.",
    ),
    // Online services
    (
        "web",
        "Online services",
        "Browse and play your library in a web browser.",
    ),
    (
        "aura",
        "Online services",
        "Serve your library over the AURA API for compatible apps.",
    ),
    (
        "mbcollection",
        "Online services",
        "Keep a MusicBrainz collection in sync with your library.",
    ),
    (
        "mbsubmit",
        "Online services",
        "Help add releases to MusicBrainz from unmatched imports.",
    ),
    (
        "ipfs",
        "Online services",
        "Share and fetch albums over IPFS.",
    ),
];

/// One of a plugin's `beet` subcommands.
#[derive(Clone, Debug)]
pub struct PluginCommand {
    pub name: String,
    pub about: String,
    /// The command takes a query (items to act on).
    pub takes_query: bool,
}

#[derive(Clone, Debug)]
pub struct PluginInfo {
    pub name: &'static str,
    pub category: &'static str,
    pub about: &'static str,
    pub commands: Vec<PluginCommand>,
    /// The default settings, as YAML (empty when it has none).
    pub defaults: &'static str,
    /// Lower-cased text the search box matches.
    pub haystack: String,
}

/// Every bundled plugin, by category then name.
pub fn plugins() -> Vec<PluginInfo> {
    let commands = plugin_registry::commands();
    let mut out: Vec<PluginInfo> = plugin_registry::defs()
        .into_iter()
        .map(|def| {
            let (category, about) = INFO
                .iter()
                .find(|(n, _, _)| *n == def.name)
                .map_or(("Other", ""), |(_, c, a)| (*c, *a));
            let cmds: Vec<PluginCommand> = commands
                .iter()
                .filter(|c| c.plugin == def.name)
                .map(|c| {
                    let cmd = (c.build)();
                    let takes_query = cmd
                        .get_arguments()
                        .any(|a| a.is_positional() && a.get_id().as_str() == "query");
                    PluginCommand {
                        name: cmd.get_name().to_string(),
                        about: cmd.get_about().map(ToString::to_string).unwrap_or_default(),
                        takes_query,
                    }
                })
                .collect();
            let haystack = format!(
                "{} {} {} {}",
                def.name,
                category,
                about,
                cmds.iter()
                    .map(|c| format!("{} {}", c.name, c.about))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
            .to_lowercase();
            PluginInfo {
                name: def.name,
                category,
                about,
                commands: cmds,
                defaults: def.defaults.trim_end(),
                haystack,
            }
        })
        .collect();
    let rank = |c: &str| {
        CATEGORIES
            .iter()
            .position(|x| *x == c)
            .unwrap_or(usize::MAX)
    };
    out.sort_by(|a, b| {
        rank(a.category)
            .cmp(&rank(b.category))
            .then(a.name.cmp(b.name))
    });
    out
}

/// Whether `p` matches every word of the search.
pub fn matches(p: &PluginInfo, search: &str) -> bool {
    search
        .split_whitespace()
        .all(|w| p.haystack.contains(&w.to_lowercase()))
}

/// The config text with its top-level `plugins:` setting replaced by
/// `names` (added at the top when missing). The rest of the file is kept
/// as it is, comments included.
pub fn set_plugins(text: &str, names: &[String]) -> String {
    let line = if names.is_empty() {
        "plugins: []".to_string()
    } else {
        format!("plugins: {}", names.join(" "))
    };
    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines.iter().position(|l| {
        l.strip_prefix("plugins:")
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
    }) else {
        let mut out = line;
        out.push('\n');
        if !text.is_empty() {
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
        return out;
    };
    // A block list continues on indented or `- ` lines (and blank ones
    // between them).
    let mut end = start + 1;
    let inline = !lines[start]["plugins:".len()..].trim().is_empty();
    if !inline {
        while end < lines.len() {
            let l = lines[end];
            if l.trim().is_empty() || l.starts_with([' ', '\t']) || l.starts_with('-') {
                end += 1;
            } else {
                break;
            }
        }
        // Leave trailing blank lines to what follows.
        while end > start + 1 && lines[end - 1].trim().is_empty() {
            end -= 1;
        }
    }
    let mut out: Vec<String> = lines[..start].iter().map(ToString::to_string).collect();
    out.push(line);
    out.extend(lines[end..].iter().map(ToString::to_string));
    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn every_bundled_plugin_is_described() {
        let all = plugins();
        assert!(all.len() > 70);
        let missing: Vec<_> = all
            .iter()
            .filter(|p| p.category == "Other")
            .map(|p| p.name)
            .collect();
        assert!(missing.is_empty(), "no description for {missing:?}");
        let fetchart = all.iter().find(|p| p.name == "fetchart").unwrap();
        assert!(fetchart.commands.iter().any(|c| c.name == "fetchart"));
        assert!(fetchart.defaults.contains("fetchart:"));
    }

    #[test]
    fn search_matches_every_word() {
        let all = plugins();
        let hits: Vec<_> = all
            .iter()
            .filter(|p| matches(p, "cover ART"))
            .map(|p| p.name)
            .collect();
        assert!(hits.contains(&"fetchart"), "{hits:?}");
        assert!(!hits.contains(&"lyrics"));
    }

    #[test]
    fn replaces_inline_and_block_plugin_lists() {
        assert_eq!(
            set_plugins(
                "directory: ~/m\nplugins: a b\nui:\n  color: yes\n",
                &names(&["a", "c"])
            ),
            "directory: ~/m\nplugins: a c\nui:\n  color: yes\n"
        );
        assert_eq!(
            set_plugins(
                "# mine\nplugins:\n  - a\n  - b\n\nlibrary: x.db\n",
                &names(&["b"])
            ),
            "# mine\nplugins: b\n\nlibrary: x.db\n"
        );
        assert_eq!(set_plugins("plugins: [a, b]", &names(&[])), "plugins: []\n");
        // `pluginsx:` is another key.
        assert_eq!(
            set_plugins("pluginsx: 1\n", &names(&["a"])),
            "plugins: a\npluginsx: 1\n"
        );
        assert_eq!(
            set_plugins("", &names(&["fetchart"])),
            "plugins: fetchart\n"
        );
    }
}
