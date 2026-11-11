BEGIN TRANSACTION;
CREATE TABLE album_attributes (
                    id INTEGER PRIMARY KEY,
                    entity_id INTEGER,
                    key TEXT,
                    value TEXT,
                    UNIQUE(entity_id, key) ON CONFLICT REPLACE);
INSERT INTO "album_attributes" VALUES(1,1,'rating','5');
CREATE TABLE albums (added REAL, album TEXT, albumartist TEXT, albumartist_credit TEXT, albumartist_sort TEXT, albumartists TEXT, albumartists_credit TEXT, albumartists_sort TEXT, albumdisambig TEXT, albumstatus TEXT, albumtype TEXT, albumtypes TEXT, artpath BLOB, asin TEXT, barcode TEXT, catalognum TEXT, comp INTEGER, country TEXT, day INTEGER, discogs_albumid INTEGER, discogs_artistid INTEGER, discogs_labelid INTEGER, disctotal INTEGER, genres TEXT, id INTEGER PRIMARY KEY, label TEXT, language TEXT, mb_albumartistid TEXT, mb_albumartistids TEXT, mb_albumid TEXT, mb_releasegroupid TEXT, month INTEGER, original_day INTEGER, original_month INTEGER, original_year INTEGER, r128_album_gain REAL, release_group_title TEXT, releasegroupdisambig TEXT, rg_album_gain REAL, rg_album_peak REAL, script TEXT, style TEXT, year INTEGER);
INSERT INTO "albums" VALUES(1700000000.25,'Ünïcode Album','Artist','','','','','','','','','',NULL,'','','',0,'',0,0,0,0,0,'Rock\␀Pop',1,'','','','','','',0,0,0,0,NULL,'','',NULL,NULL,'','',2001);
CREATE TABLE item_attributes (
                    id INTEGER PRIMARY KEY,
                    entity_id INTEGER,
                    key TEXT,
                    value TEXT,
                    UNIQUE(entity_id, key) ON CONFLICT REPLACE);
INSERT INTO "item_attributes" VALUES(1,1,'mood','happy');
INSERT INTO "item_attributes" VALUES(2,1,'rating','5');
INSERT INTO "item_attributes" VALUES(3,2,'rating','5');
CREATE TABLE items (acoustid_fingerprint TEXT, acoustid_id TEXT, added REAL, album TEXT, album_id INTEGER, albumartist TEXT, albumartist_credit TEXT, albumartist_sort TEXT, albumartists TEXT, albumartists_credit TEXT, albumartists_sort TEXT, albumdisambig TEXT, albumstatus TEXT, albumtype TEXT, albumtypes TEXT, arrangers TEXT, arrangers_ids TEXT, artist TEXT, artist_credit TEXT, artist_sort TEXT, artists TEXT, artists_credit TEXT, artists_ids TEXT, artists_sort TEXT, asin TEXT, barcode TEXT, bitdepth INTEGER, bitrate INTEGER, bitrate_mode TEXT, bpm INTEGER, catalognum TEXT, channels INTEGER, comments TEXT, comp INTEGER, composer_sort TEXT, composers TEXT, composers_ids TEXT, country TEXT, day INTEGER, disc INTEGER, discogs_albumid INTEGER, discogs_artistid INTEGER, discogs_labelid INTEGER, disctitle TEXT, disctotal INTEGER, encoder TEXT, encoder_info TEXT, encoder_settings TEXT, format TEXT, genres TEXT, grouping TEXT, id INTEGER PRIMARY KEY, initial_key TEXT, isrc TEXT, label TEXT, language TEXT, length REAL, lyricists TEXT, lyricists_ids TEXT, lyrics TEXT, mb_albumartistid TEXT, mb_albumartistids TEXT, mb_albumid TEXT, mb_artistid TEXT, mb_artistids TEXT, mb_releasegroupid TEXT, mb_releasetrackid TEXT, mb_trackid TEXT, mb_workid TEXT, media TEXT, month INTEGER, mtime REAL, original_day INTEGER, original_month INTEGER, original_year INTEGER, path BLOB, r128_album_gain REAL, r128_track_gain REAL, release_group_title TEXT, releasegroupdisambig TEXT, remixers TEXT, remixers_ids TEXT, rg_album_gain REAL, rg_album_peak REAL, rg_track_gain REAL, rg_track_peak REAL, samplerate INTEGER, script TEXT, style TEXT, subtitle TEXT, title TEXT, track INTEGER, trackdisambig TEXT, tracktotal INTEGER, work TEXT, work_disambig TEXT, year INTEGER);
INSERT INTO "items" VALUES('','',1700000000.5,'Ünïcode Album',1,'Artist','','','','','','','','','','','','Artist','','','','','','','','',0,63999,'CBR',0,'',1,'',0,'','','','',0,0,0,0,0,'',0,'','','','MP3','Rock\␀Pop','',1,NULL,'','','',1.044897959183673564e+00,'','','','','','','','','','','','','',0,1700000001.0,0,0,0,X'4172746973742FC39C6EC3AF636F646520416C62756D2F3031204F6E652E6D7033',NULL,NULL,'','','','',NULL,NULL,NULL,NULL,44100,'','','','One',1,'',0,'','',2001);
INSERT INTO "items" VALUES('','',1700000000.5,'Ünïcode Album',1,'Artist','','','','','','','','','','','','Artist','','','','','','','','',0,63999,'CBR',0,'',1,'',0,'','','','',0,0,0,0,0,'',0,'','','','MP3','','',2,NULL,'','','',1.044897959183673564e+00,'','','','','','','','','','','','','',0,1700000001.0,0,0,0,X'4172746973742FC39C6EC3AF636F646520416C62756D2F30322054776F2E6D7033',NULL,NULL,'','','','',NULL,NULL,NULL,NULL,44100,'','','','Two',2,'',0,'','',2001);
INSERT INTO "items" VALUES('','',1700000000.5,'',NULL,'','','','','','','','','','','','','Solo','','','','','','','','',0,63999,'CBR',0,'',1,'hello',0,'','','','',0,0,0,0,0,'',0,'','','','MP3','','',3,NULL,'','','',1.044897959183673564e+00,'','','','','','','','','','','','','',0,1700000001.0,0,0,0,X'4E6F6E2D416C62756D2F536F6C6F2F53696E676C6520C2BD2E6D7033',NULL,NULL,'','','','',NULL,NULL,NULL,NULL,44100,'','','','Single ½',0,'',0,'','',0);
CREATE TABLE migrations (
                    name TEXT NOT NULL,
                    table_name TEXT NOT NULL,
                    PRIMARY KEY(name, table_name)
                );
INSERT INTO "migrations" VALUES('multi_genre_field','items');
INSERT INTO "migrations" VALUES('multi_genre_field','albums');
INSERT INTO "migrations" VALUES('lyrics_metadata_in_flex_fields','items');
INSERT INTO "migrations" VALUES('multi_remixer_field','items');
INSERT INTO "migrations" VALUES('multi_lyricist_field','items');
INSERT INTO "migrations" VALUES('multi_composer_field','items');
INSERT INTO "migrations" VALUES('multi_arranger_field','items');
INSERT INTO "migrations" VALUES('relative_path','items');
INSERT INTO "migrations" VALUES('relative_path','albums');
INSERT INTO "migrations" VALUES('remove_inherited_artpath','items');
INSERT INTO "migrations" VALUES('instrumental_lyrics_in_flex_field','items');
CREATE INDEX item_attributes_by_entity
                    ON item_attributes (entity_id);
CREATE INDEX idx_item_album_id ON items (album_id);
CREATE INDEX album_attributes_by_entity
                    ON album_attributes (entity_id);
COMMIT;
