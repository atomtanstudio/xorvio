# Xorvio in Rust and Tauri

Xorvio began as a Swift/SwiftUI app for the Mac App Store and was rewritten in Rust + Tauri as open source. This page is the plan and the contract between the parts.

## Decisions

- **Open source, no App Store.** No StoreKit, no Pro tier: every feature is free. The license is GPL-3.0-or-later (it lets us bundle GPL ffmpeg builds).
- **One app.** Xorvio Personal's add-on support becomes part of the main app. Add-ons speak the Stremio add-on protocol. The app ships with no add-ons installed and never names or recommends a particular one; people paste a manifest link in Settings › Add-ons.
- **Same look.** The interface is rebuilt in React + TypeScript to match the SwiftUI app's screens: the floating sidebar, the full-window still hero, Live TV after TiviMate's layout, poster grids, title pages and the player. Backdrops never move (no pan or zoom).
- **New: recording.** Record what's on now, or schedule a programme from the guide, a watchlist card or search. The recorder runs in the Rust backend with ffmpeg.
- **New: media servers.** Plex (signed in through plex.tv, or by address), Jellyfin and Emby are sources like any other: their libraries become categories, their films and series play straight from the server's files through the media gateway, and what's been watched goes both ways (merged after each refresh, sent back as progress is saved). Xorvio keeps a server's token, never a password, and the token never reaches the catalog or the interface: titles keep the server's keys, and artwork goes through a `xorvio-server://` address the app fetches with the token. `Scripts/test-servers/` starts throwaway Docker servers with generated test media for the ignored integration tests.
- **New: Trakt** (Settings › Trakt; `crates/xorvio-core/src/trakt/`). Connecting uses Trakt's device code (a code entered at trakt.tv/activate). The Trakt app's client id comes from the build (`XORVIO_TRAKT_CLIENT_ID` in the environment when it's compiled) or from Settings (kept in the keychain). No client secret is used: the code sign-in, token refreshes and revoking need only the client id, and Trakt asks that desktop apps never send a secret (see developer.trakt.tv/docs/pkce). Trakt lets development apps redirect only to this computer, so the app is registered with `http://localhost` as its redirect URI: connecting with a code never redirects, but refreshing a token names it, so another registered address goes in Settings or `XORVIO_TRAKT_REDIRECT_URI`; the tokens are kept in the keychain too and never reach logs, errors or the interface. The player scrobbles films and episodes (start, pause, stop; Trakt marks a title watched itself at 80%). Marking watched or unwatched by hand, taking a card off Continue Watching and saving to the watchlist are sent in the background. A sync (shortly after launch, every 15 minutes, after connecting, or by hand) asks `/sync/last_activities` what changed, keeps what Trakt says in library tables, and applies it like a media server's watched state: watched marks on every matching film and episode, places left part-way as resume points (as add-on titles when the catalog hasn't the title), Xorvio's own newer progress kept, nothing ever removed because of Trakt. Xtream episodes pick up watched marks when a series is opened and its episodes are fetched. The watchlist's films and series go both ways, told apart from titles never synced by what both sides had after the last sync. Titles are matched by IMDb or TMDB id, or by title and year, looked up on Trakt once when they have no ids. The demo library is never sent.
- **New: Trakt rows on Home** (`crates/xorvio-core/src/trakt/lists.rs`, Customize Home › From Trakt). Trakt's charts (Top 10 trending, popular, most watched this week, box office), any public list (found by name, or by pasting its trakt.tv address, such as a member's IMDb Top 250), the connected account's own and liked lists, and its recommendations become rows. Charts and public lists need only the app's client id, but Home's rows ask Trakt nothing until an account is connected or Settings › Trakt › On Home ("Show Trakt's charts and lists before connecting", off by default) says to (`Core::set_trakt_rows_without_account`). Each title shows as the sources' copy (matched as the sync matches: IMDb or TMDB id, else title and year), else, when an enabled add-on can describe and play that kind, as an add-on title with Trakt's artwork; when add-ons lead, as the add-on title wherever there's one, as on Home's add-on rows, so its page lists every add-on's copies and then the sources' own; hidden, locked and profile-excluded titles are left out, and Trakt's order is kept. Charts show as a Top 10; lists and recommendations as up to 40 posters. Each row's Show All opens the whole of it (up to 1,000 titles) as a grid, with how many of Trakt's titles can be watched, and for a list each title's place in it. What Trakt sends is kept for half an hour (and used for longer when Trakt can't be reached). Home layouts went to version 2: every layout gains Top 10 Movies and Top 10 Series on Trakt (after Continue Watching when add-ons lead, after Tonight on TV otherwise); removed, they stay removed.
- **New: a Home row for each library** (`src/lib/homeLayout.ts`, `LibraryRow` in `src/screens/Home.tsx`). Every media folder and media server (Plex, Jellyfin, Emby) has a row of its own, named after the source, its films and series together, newest first. It's there from when the source is added (after Continue Watching and the watchlist, when a layout starts with them), takes the source's new name, and goes when the source is removed; Customize Home switches it off rather than removing it, so it doesn't come back. Show All opens Movies (or Series) as that source's, which loads every title as it scrolls.
- **New: frames for videos without artwork** (`crates/xorvio-media/src/stills.rs`, the `fill_posters` command). A media folder's film with no picture of its own (no poster beside it, none found on TMDB) gets a frame of its video as its card, taken about 15% of the way in (no further than 90 seconds), 360 lines high, made with ffmpeg at low priority, three at a time, only for titles a list is showing, and kept in `Stills/` named for the file's path, size and change time; the catalog keeps its address (`xorvio-still://`) as it keeps TMDB's posters, so lists show it at once from then on, and the film's page uses it as its backdrop. A media folder's video with no year and no ids is never looked up on TMDB (it isn't a film TMDB would know), and a rescan forgets TMDB posters guessed for those by name. A media folder's or media server's film titles are shown as they are: "XTC - Dear God" isn't taken for a provider's "EN - " tag.
- **New: a media folder's own library page** (`crates/xorvio-core/src/media_library.rs`, `src-tauri/src/media_library.rs`, `src/screens/MediaLibrary.tsx`). A media folder's Home row's Show All, or Open Library in Sources, opens its folders and videos as they are on the disk, with a way back up through the folders above. Each video's file is read once in the background (ffprobe at low priority, four at a time: length, picture size, sound, size on disk, kept in the catalog's `file_facts`), so the page can show GIFs, animations or videos, under a minute or a minute or longer, newest, by name, longest or largest, in one folder or everything inside it. A GIF is a GIF made into a video as GIF sites save them: silent, 30 seconds or less, and no more than 540 pixels on its longer side or under 600 kbit/s; an animation is any other short silent clip (AI clips are 720p or more at a few Mbit/s); anything with sound or longer is a video. Until a file is read the page says how many are left and a filter can't place it.
  Edit puts a checkbox on each video and folder: a click ticks one, Shift-click a run and ⌘-click (Ctrl elsewhere) one more, Select All everything the filters show on every page, Esc leaves. Move to… takes them to another folder of the same source (a new one can be made there), Delete moves them to the folder's Recently Deleted: `.xorvio-recently-deleted/<when>-<id>/` inside it, hidden from the scan, with a `<when>-<id>.json` of what went, so thousands go in an instant even on a network share (which has no Trash). Recently Deleted puts each deletion back where it was or deletes it for good, and anything there 30 days goes by itself. Folders can be made, renamed, moved and deleted, and a video renamed. A video's pictures, .nfo and subtitles named after it go with it (not those of another video with a longer name, "Clip-2.jpg" stays with "Clip-2.mp4"), and so does anything named for its whole file name ("Clip.mp4.json", "Clip.mp4.xmp", as downloaders keep); a name already taken there, in any case, makes "Clip (2).mp4", and nothing is ever renamed over something else. Nothing outside the source's folders is touched: paths are checked against them, nothing hidden, no "..", and the folder a change acts in must lead back inside them once links are followed. The disk work is planned off the async threads with each folder listed once. A network share often lets a login read a folder it can't change (files a server's own apps wrote): then nothing goes, the page says which folder is read-only and how to fix it, and the message stays until it's closed; a deletion that took nothing leaves no folder behind in Recently Deleted (one an hour old without its list is tidied away). A video moved or renamed keeps its progress, watchlist place, history, frame and file's facts under the id the next scan gives it; the catalog follows each change at once, and the folder is read again in the background (once more if it was being read already). With a filter on, Select All takes only the videos it shows, not the folders (which aren't filtered).
- **A file a folder lists stays even when it can't be looked at** (`keep_missed` in `crates/xorvio-core/src/sources/folder/scan.rs`). A Mac's network share can list a name it then can't find: a file written to the server in decomposed Unicode ("Jónsi", as a Mac disk keeps names), which the Mac's SMB client looks up composed, so it's only found while the Mac still has it cached. The scan used to drop whatever it couldn't stat, so those films went from the library on one scan and came back on another, losing their file's facts each time. Now a listed file keeps what the last scan knew of it (or comes with nothing known: no size, no date added); a link that leads nowhere is still left out. The cure for playing them is on the server: their names written composed (`convmv -f utf8 -t utf8 --nfc`).
- **New: trailers behind title pages** (`src/screens/TitleTrailer.tsx`, `crates/xorvio-media/src/gateway/trailer.html`). A film or series page with a trailer (TMDB's, or an add-on's) plays it behind the title as Netflix does, loading as the page opens and fading in the moment YouTube plays (under a second once YouTube's player is cached): muted until the sound button is pressed (then on for the session), enlarged past YouTube's title and logo, faded back to the still backdrop 2.5 seconds before the end so YouTube's suggestions never show, with a button to play it again. It stops the moment anything else plays (and starts again after), pauses while the window is out of sight, and gives up quietly on a trailer YouTube won't play. YouTube's player refuses pages that aren't served over http, as the app's aren't, so the media gateway serves the page that holds it (`/{token}/y/trailer/player.html?v=…`, sending only its origin as the referrer). Settings › Profiles and Home › Trailers turns it off (on by default); the Mac's Reduce Motion keeps it off. The preview plays one with `?trailer=<YouTube id>`.
- **New: TMDB** (Settings › Details; `crates/xorvio-core/src/tmdb/`). With the person's own free key (an API key or a read access token from themoviedb.org › Settings › API, checked with TMDB before it's kept in the keychain) or one built in (`XORVIO_TMDB_KEY` when compiled), films and series get title logos, wordless backdrops, cast with photos, the trailer, seasons with posters, episode stills, synopses and names (for episodes a source only numbers), More Like This from TMDB's recommendations the sources carry, and a page for each person (photo, biography, their titles in the sources). Titles are matched by the TMDB id their source gives, else their IMDb id, else their name and year (searched with the year, then without; taken only when the name is the same and the year within one), which is all that's sent; the demo library never is. What TMDB says is kept in the catalog (`tmdb_cache`, `tmdb_matches`; catalog schema 4) for 30 days, misses for 14, and laid over the source's details as they're read: over an IPTV provider's, under a media server's. TVmaze is asked about series only when TMDB can't help. Settings › Details › TMDB switches it off altogether (`Core::set_tmdb_enabled`, checked in `Tmdb::key`, which every request goes through), whichever key there is; what it said before still shows. A key TMDB later turns down stops every request until one is saved again; when TMDB can't be reached, a title waits 10 minutes before it's asked about again. Settings and About carry TMDB's notice ("This product uses the TMDB API but is not endorsed or certified by TMDB."). Add-on titles are looked up by their IMDb id, with TMDB laid over the add-on's details as over a provider's, and More Like This and cast pages work from them too. Title pages also get TMDB's users' score (once ten have voted) and its YouTube videos (trailers, teasers, clips; an add-on's IMDb score too), and titles kept in an earlier form are asked for again once. Lists fill posters a source left out from TMDB: only for titles on screen, after scrolling stops, a dozen at a time and one request after another, kept by catalog id (`tmdb_posters`, catalog schema 5) so the next list shows them at once.
- **Browsing films and series** (`crates/xorvio-core/src/titles.rs`). A film several sources (or categories) carry is one poster in Movies: copies share a TMDB id, else an IMDb id, else a title and year (the catalog's `version_key`, indexed, so a page of 120 at 31,000 films takes 20–40 ms). The first copy in the list's order stands for the film; its page lists every copy in Versions, one chosen to play. Movies and Series filter by source and by what's been watched (All, Unwatched, In Progress: a film by any copy's progress; a series is under way once an episode is, and done when every episode the catalog lists is), with the library's progress passed into the query as ids. Films and series can be hidden from the library, with every copy of a film, and Settings › Hidden & Locked shows them again. Movies and Series' categories can be pinned to Home as rows, renamed, hidden and locked from their category list.
- **People without TMDB** (`crates/xorvio-core/src/wikidata.rs`): a person's page comes from Wikidata's filmography (CC0, no key) when there's no TMDB key or TMDB doesn't know them. Only the name is sent, never for the demo library. A photo from Wikimedia Commons is shown only with its credit (author and licence from Commons' `extmetadata`, asked with the file's name; `PersonDetails::photo_credit` and `photo_page`); one Commons can't credit is left out. One request a second, kept for 30 days (a name it doesn't know for 14) in `wikidata_people`. Credits are matched by TMDB or IMDb id, else by name and year.
- **Diagnostics** (Settings › Privacy & Diagnostics; `src-tauri/src/diagnostics.rs`): a `tracing` layer keeps the latest 2,000 events in memory, warnings and errors always and Xorvio's own detail on request, each redacted as it's kept, with area and level filters. Exporting saves a text file where the person chooses, with every source's login masked once more. **Live TV latency** (Settings) sets how close to live hls.js and mpegts.js play unbuffered channels; WebKit's own HLS player sets its own.
- **Plain-http artwork** goes through the app (`crates/xorvio-core/src/images.rs`, the `xorvio-image` scheme; `src/lib/images.ts` makes the addresses). The web view asks for a page's plain-http images over https instead, and many providers' image servers speak only http, so their posters and logos showed as placeholders. The app fetches them as given, eight at a time, and passes on only what really is an image.
- **Library profiles** (`crates/xorvio-core/src/profiles.rs` and `classify.rs`; switched at the foot of the sidebar, edited from there or Settings › Profiles and Home). A profile ("English only", "Action") chooses languages, favourite genres (optionally the only ones shown), live TV, films and series, whether adult categories are hidden, and categories shown or left out by hand. Category names are read for languages, genres and adult content as the Swift app's CategoryClassifier did ("US|", "QC - FILMS", "TURKISH SERIES", "EN - ACTION"). The active profile joins hidden and locked items in `Core::excluded`, so every list follows it: Live TV and the guide's search, Movies, Series, search, Tonight on TV, On Now, Top 10, genres, More Like This, person pages, Continue Watching and the watchlist as shown (the watchlist itself keeps everything). A language profile also leaves out titles tagged with another country ("Title (JP)"). Add-on titles have no categories, so only kinds, their genre fields and country tags apply to them; languages can't. The parental PIN locks what it locks whatever the profile, and a profile can't show a locked category. Everything, the profile there is before any other is made, leaves nothing out. Profiles and the active one are kept in library.sqlite (schema 9).
- **Customize Home** (`src/lib/homeLayout.ts`, `src/components/HomeLayoutSheet.tsx`). Each profile has its own Home rows, one arrangement for when the sources lead Home and one for when add-ons do: shown or hidden, in any order, with rows for genres, categories and single add-on catalogues added by hand. The interface defines the rows and the standard layout (Home as it was, plus a row per favourite genre, New Releases and New Episodes); the backend keeps each layout as the interface sent it. When the sources carry channels but no films or series, the standard layout leads with the live rows and the hero features what's on now. Home shows a banner for a source that couldn't refresh (Review goes to Sources) and for the demo library alone, and its arrow keys move a highlight across the hero, rows and cards as an Apple TV remote does (`src/screens/homeKeyboard.ts`).
- **New episodes** (`crates/xorvio-core/src/new_episodes.rs`, `src-tauri/src/reminders.rs`). Series on the watchlist and series someone's caught up on are looked at every hour, or once a day when their newest episode aired over two months ago: each one's episodes are fetched again (one series at a time, spaced out; add-on series from their add-on, by the app), and anything newer than last time gets a "New Episode" badge until the series is opened, a place in Home's New Episodes row (under Continue Watching) until it's watched, and a notification (Settings › Reminders, "Tell me when new episodes arrive") whose click opens the series and whose Play button plays the first new one. What was seen is kept in library.sqlite's preferences, so no schema change. Continue Watching labels episodes not started "New Episode" (aired in the last two weeks), "Up Next", "Latest Episode" or "Start Watching" (`ContinueItem::reason`).
- **Preview clips** (Settings › Profiles and Home, off by default; `src/components/HeroPreview.tsx`). A short muted excerpt of the featured film's own stream fades in behind the hero after a few seconds, only for files the web view plays as they are, and only while nothing else plays; anything that starts playing stops it first. Trailers aren't used: TMDB's are YouTube videos, which play only in YouTube's own player. Otherwise backdrops never move.
- **The menu bar** (`src-tauri/src/menu.rs`, `src/lib/commands.ts`): Xorvio, File, Edit, View, Go, Playback, Window and Help as the Swift app's Commands had them (no app menu on Windows and Linux; Settings and Quit go in File, About in Help). Xorvio's own items reach the interface as a `menu` event naming the item, sent to the main window, or for Playback to the window the video is in, and are done there by the same functions as the buttons and keys; each window reports what can act now (`set_menu_state`) and the rest are disabled. On a Mac the page sees a shortcut before the menu, which acts only on one the page lets pass, so key handlers leave the menu's shortcuts alone (`isMenuShortcut`), except the ⌘-arrows and ⌥Space, which the web view would scroll with first: the interface does those itself outside text fields. The browser preview, with no menu bar, catches them all itself.
- **First run** (`src/screens/Onboarding.tsx`): with no sources and the welcome not yet seen, a welcome with the content notice, then a choice of source or the demo, then the library as it loads, with languages and adult categories to leave out, kept as a "My Library" profile. Skipping shows Home's own empty state.

## Layout

```
Cargo.toml              Rust workspace
crates/
  xorvio-model/         Shared serde types (the JSON contract with the interface)
  xorvio-core/          Sources (M3U, Xtream, XMLTV, Plex, Jellyfin, Emby, demo), SQLite catalog + library, search, secrets
  xorvio-addons/        Stremio-protocol add-on client and registry
  xorvio-media/         Media gateway (local HTTP proxy, HLS rewrite, ffmpeg remux) and the recorder
  xorvio-cast/          Chromecast and Google TV: mDNS discovery and Cast v2 (TLS, port 8009)
src-tauri/              The Tauri app: commands, events, windows, app state
src/                    The interface (React + TypeScript + Vite)
Docs/RustRewrite.md     This page
```

Data lives in Tauri's app data folder (`~/Library/Application Support/com.xorvio.desktop` on macOS, deliberately apart from the Swift app's `Xorvio` folder so the two never share files): `catalog.sqlite` (rebuildable provider data), `library.sqlite` (user-authored state), `sources.json` (non-secret source settings), `addons.json`, `recordings.sqlite`, `settings.json`, `sync-state.json` (this computer's sync id and what it last saw; never backed up) and `Backups/` (automatic backups, unless another folder is chosen). Secrets (playlist URLs, Xtream logins, media server tokens, add-on links) go to the OS keychain through the `keyring` crate.

## Backup and sync

**Backups** (Settings › Backup; `crates/xorvio-core/src/backup/`, `src-tauri/src/backup.rs`). Back Up… saves one `Xorvio Backup <date>.xorvio-backup`, a zip of `manifest.json` (format, app version, when, the computer's name, the files, counts of sources, add-ons, watchlist, progress, history, favorites, groups, item edits and recordings), copies of `library.sqlite` and `recordings.sqlite` made by SQLite while they're open (`VACUUM INTO`), and `settings.json`, `sources.json` and `addons.json`. The catalog is never in one. Logins and links (every source's and added guide's keychain entry, add-on links, Trakt's connection and app, the TMDB key) go in only when the person sets a passphrase for that backup: a key from Argon2id (64 MiB, 3 passes) seals them with XChaCha20-Poly1305 in `secrets.sealed`, header authenticated, passphrase never kept. Without one the person is told sources will need their logins again elsewhere. Automatic backups (off, daily or weekly) run from a 15-minute loop, never within ten minutes of a recording starting, never with logins, into `Backups/` or a chosen folder, named `Xorvio Backup <date> <time> (<computer>).xorvio-backup`; the latest 7 of this computer's are kept, and backups made by hand are never pruned.

**Restoring** checks the backup (manifest, a newer format or library schema turned away, databases pass `quick_check`, JSON parses, the passphrase opens the logins, or a plain message says it doesn't) and stages its files in `restore-pending/`, logins sealed again with a random key the keychain holds meanwhile, then restarts (`AppHandle::request_restart`). Restoring is refused while something records; scheduled recordings are named in the confirmation. At the next launch, before anything opens the data, `apply_pending_restore` sets this computer's files (with the databases' `-wal`/`-shm`) aside in `restore-previous/`, records which, copies the backup's in, keeps the keychain's earlier values sealed beside them and writes the logins; renaming `restore-pending/` is the commit. Any failure, or the app closing part-way (`restore-pending/applying` is still there at the next launch), puts every file and keychain entry back; undoing can itself be cut off and run again. The outcome is a toast after the restart and a line in Settings › Backup. Catalog data for sources the backup doesn't have stays; sources new to this computer refresh as usual.

**Sync** (Settings › Sync; `crates/xorvio-core/src/sync.rs`, `store/library/sync.rs`) goes through a folder the computers share (iCloud Drive, Dropbox, Syncthing, a NAS share); there's no server. Each computer writes `<device id>.xorvio-sync`, gzip-compressed JSON: per table its key and change-time columns, its rows as arrays, and its tombstones (key values and when). It's written to a dot-named temporary file and renamed, and only when its contents changed. The other computers' files are read defensively: a file that doesn't decompress or parse (still arriving) is skipped until it does, a conflict copy is merged like any other, and a copied data folder writing as this computer is noticed and this computer takes a new id. Merging is newest wins per row: a row replaces this computer's when it changed later, a tombstone deletes it when the deletion was later, a row beats a tombstone from the same second, and two rows from the same second are settled by their contents so every computer ends the same. Merges don't count as local changes, and tombstones are kept 180 days. It runs at launch, every 10 minutes, and 45 s after the library goes quiet following a change (at most 5 minutes after it); errors go to Settings › Sync in plain words and the log, never a toast.

Synced tables are the list `SYNCED_TABLES` in `store/library/sync.rs` (a table, its key columns, its change-time column; rows whose `source_id` this computer doesn't have are left out where a table names one): `watchlist`, `progress` (by its own `updated_at`, so the newer position wins, as `upsert_progress` does), `history` (keyed by item and time; its row id stays local) and `history_titles`, `favorites`, `favorite_lists`, `item_edits` (names, hiding and lock flags), `guide_overrides`, `catchup_offsets`, `preferred_versions`, and `profiles` and `home_layouts` (listed by name for the profiles work, passed over until those tables exist). Not synced (`LOCAL_TABLES`): preferences (the parental PIN, the active profile), Trakt's tables, added guides, dismissed shows, reminders sent. A test fails when a library table is in neither list, when a synced key isn't the table's primary key, or when a delete leaves no tombstone. Library schema **10** adds `sync_tombstones`, `sync_meta` (a count of local changes the sync loop watches), `sync_untracked` and an index on history's key; each time the library opens, every synced table gets its `changed_at` column (`ALTER TABLE … DEFAULT 0`, so older rows count as oldest) and three triggers that stamp change times, write tombstones on any delete and clear them on re-insert, so every delete path is covered. Removing a source or an added guide tidies its rows without tombstones (they aren't synced, and another computer may still have them), and guide choices naming a guide this computer lacks are ignored. Item ids carry their source's id, so titles match across computers that share sources: set up the second computer by restoring a backup, then turn sync on on both; add-on titles match anyway.

## Playback

The web view can't play everything (MKV, AC-3/DTS audio, raw MPEG-TS), so all playback goes through the **media gateway**, a small HTTP server on `127.0.0.1` with a per-launch random token:

| Stream | How it plays |
| --- | --- |
| HLS (`.m3u8`) | hls.js; the gateway rewrites playlists so every segment and key also comes through it (headers, CORS) |
| Raw MPEG-TS / FLV (live) | mpegts.js over the gateway's pass-through proxy |
| MP4 / WebM / MOV with web codecs | the `<video>` element directly, with range requests passed through |
| Anything else (MKV, AVI, AC-3/DTS/TrueHD audio, MPEG-2) | ffmpeg remuxes to MPEG-TS for mpegts.js, copying what the web view's Media Source decodes (H.264, HEVC, AAC, MP3, and AC-3/E-AC-3 where the interface reports them at launch) and converting the rest: video to H.264 (VP9 and AV1 too, which MPEG-TS can't carry here, made 1080p when they're taller so encoding keeps up), audio to AAC in a standard 5.1/stereo/mono layout (WebKit refuses ffmpeg's custom channel maps); seeking restarts ffmpeg at an offset. MP4 and WebM files with VP9 or AV1 play directly where the interface reports the `<video>` element decodes them (AV1 needs the hardware to) |

`ffprobe` decides between direct play and remux for files and VOD; live channels go by the extension and the source's live format. Gateway URLs never contain credentials; the gateway keeps the real target (`StreamTarget`) in memory under a random id.

A native engine (libmpv in a child view under a transparent web view) can be added later behind the same `PlaybackMode` switch if web playback falls short.

Around the player:

- **Now Playing and media keys** come from WebKit itself: while the video plays it makes Xorvio the Now Playing app and passes play/pause, next and previous (keyboard, Control Center) to the page's `navigator.mediaSession`, which `src/player/mediaSession.ts` fills in (title, episode or programme, artwork, position) and handles. Next and previous change channel, or play the next episode.
- **Open in another player**: IINA, VLC and Infuse, found by bundle id through Launch Services (`src-tauri/src/players.rs`), get a gateway address of their own for the original stream (never the remux), which lasts until Xorvio quits. VLC gets a playlist that starts where the film was; IINA and Infuse start at the beginning.
- **Pop-out player** (`src-tauri/src/popout.rs`, `src/player/PopOut.tsx`): the player's pop-out button (⌥⌘P) moves the video into a window of its own, a second web view running the interface in player-only mode, and the same button there docks it back. The stream stops in one window before the other starts it, from the same moment, so a provider that allows one connection never sees two; while it's popped out, what's chosen in the main window plays there. Closing the pop-out saves where it got to and stops it. It can stay above other windows (Settings › Playback, or its options menu).
- **Picture subtitles** (PGS from Blu-rays, VobSub from DVDs, DVB) can't be text, so choosing one restarts the remuxed stream with ffmpeg drawing it into the video (`RemuxPlan::burning`), which means encoding the video; the pictures are scaled to the video's size when their canvas differs (1080p subtitles on 720p video). Choosing another track, or Off, goes back to copying. They're never turned on by the preferred subtitle language, since that would encode without asking.
- **Thumbnails while scrubbing** (`crates/xorvio-media/src/gateway/thumbnails.rs`, `src/player/seekbar.tsx`): hovering or dragging a film's, episode's or recording's seek bar shows the frame there once the title's thumbnails are made, and the time alone until then. The first time a title plays, ffmpeg makes them in the background at a low priority: a tile every 2–600 s (the shortest step that keeps it within 400 tiles, 150 for a title on a server), each a short ffmpeg that seeks to its moment and decodes only the keyframe there on one thread (`-noaccurate_seek -skip_frame nokey -threads 1`, about six times quicker for long-GOP video), 160 pixels wide in the picture's shape, joined by the `tile` filter into JPEG sheets of 10 × 10. They're kept once per title in the data folder's `Thumbnails` (300 MB at most, least recently used first) and served by the gateway (kind `p`). Tiles are in the title's own time, so a remuxed stream restarted at `start_offset_seconds` shows the same ones. Live channels, recordings still being made and HLS have none.

  A remote title's need a connection beside the one playing. They're made only while nothing else plays beyond the title's own stream (no other stream, no address handed to another player, nothing on a TV) and its source allows a second: a provider only when it reports more than one stream at a time and no recording from it is running or starting within two minutes; a media server always; an add-on's link unless it's on a provider's host (then as that provider). Both are asked again before every tile, so something else starting stops them. The first tile waits until the title has played for 15 s (out of the way of the player starting it), tiles are a second apart, and ffmpeg reads the title through a gateway feed that lets one request reach the server at a time (ffmpeg opens the next request before closing the last when it seeks; the feed closes the last first). Otherwise the title has none: building them from the stream as it plays would only cover what's been watched, not where someone scrubs to, and would make the ffmpeg serving the player decode video it otherwise copies.
- **AirPlay** is offered for native and HLS streams; Media Source streams (mpegts.js) have no address a receiver could play.
- **Reconnecting** (`src/player/recovery.ts`, the rules in `playerRules.ts`): a stream that drops (an engine error, a live stream ending, nothing arriving for 12 seconds on a channel or 30 on a film, a film ending minutes early) is opened again with "Reconnecting… attempt n of m" and Try Now. A live channel that was playing tries 8 times over about two minutes (2, 3, 5, 8, 12, 20, 30 and 30 seconds), one that never started twice, a film three times; a stream that then plays steadily for 30 seconds counts from the start again. Each attempt releases the dropped stream first and asks `play` again, so the provider sees one connection; a film or episode carries on where it first dropped (a remuxed one is restarted there, with the audio and drawn-in subtitles it had). The gateway keeps the last error status the player got for each stream (`stream_failure`: the provider's 403 or 404 passed through, its own 502), since the web view's players don't say, and failures are put into words by kind: offline, sign-in (401/403, which for a live channel that played is worth retrying: a connection still counted), not found, unsupported, a drop, a stall. Their cards suggest what to do, with Try Again, Open Sources (Choose Another Copy for add-on titles) and Close.
- **Live TV in the player** (`src/player/LiveOverlays.tsx`, `zap.ts`): keys like a remote. ↑ and ↓ (and Page Up/Down, ⌘↑/⌘↓) are channel up and down, the next and previous channel in the list (⌥↑/⌥↓ is the volume, and plain ↑/↓ over a film); up and down show the channel at once and open its stream 350 ms later (600 ms while zapping quickly), so channels passed on the way never open a provider connection. Return opens a channel list over the video on the channel's category (← and → change group: every channel, Favorites, groups, categories), with logos and what's on; ↑/↓ move and Return watches, and up and down then move through that list. Digits typed gather for 1.4 seconds and tune that channel number (the playing source's first), or say "No channel N". G shows a Now/Next card (it also flashes for 5 seconds after each channel change) with From Start where the channel has catch-up, Remind Me for the next programme and Favorite. ⇧←/⇧→ jump a minute, P is Picture in Picture, C switches to another copy of an add-on title (the copies list from the same moment, the copy playing marked; closing it plays on). The actions are functions in `src/player/controls.ts` for the menu bar.
- **Audio and subtitles for a series**: the audio language and subtitles (or Off) picked while watching an episode are kept in the library's preferences (`track_choice:<series id>`, `track_choices.rs`; local, not synced) and chosen for its other episodes: a remuxed episode starts with that audio track, a web view's own track is switched, and subtitles go to the same language and name, else the same language without forced ones. Picture subtitles are never drawn in by themselves.
- **Skip times** (`crates/xorvio-core/src/segments.rs`, `src/player/nextEpisode.ts`): an episode's intro, recap and end credits come from IntroDB (introdb.app, community-timed, no account), asked with only the series' IMDb id and the episode's number, once a session each. The player offers to skip them, or skips them itself (Settings › Playback › Intros and recaps). Settings › Playback › Skip times (on by default) stops every request (`Core::set_intro_db_enabled`).
- **Docked and mini**: the video docked in Live TV's header has play/pause, mute, pop out, full screen and stop over its foot on hover. The mini player has a strip under the picture (title, play/pause, pop out, stop), and can be dragged to any corner, where it settles and stays (`player.miniCorner`).
- **Multiview** (`src/player/Multiview.tsx`, `multiviewStore.ts`, `multiviewRules.ts`) shows two to four live channels in the main window, where the player shows. Each tile is a gateway stream and a provider connection of its own, asked for with `play(request, direct: true)` so it never plays from the live buffer, and fails on its own (with Retry). One tile is heard: click it or press 1–4. Two sit side by side, three as one large tile with two stacked beside it, four in a 2×2 grid, all 16:9; a tile can fill the view (Esc returns). It opens from Add to Multiview (channel menus, Live TV's header), the player's button for live channels or ⇧⌘M; a live channel playing carries its stream into the first tile. Closing it, or removing tiles down to one, carries the tile heard on in the player and releases the rest. Before a tile is added, `connection_warning` says when the provider's `max_connections` would be passed, counting the source's tiles and the recordings running. Now Playing follows the tile heard; next and previous move the sound along, and pausing pauses every tile. `node src/player/multiviewRules.test.ts` checks the layout and sound rules.
- **Chromecast and Google TV** (`crates/xorvio-cast/`, `crates/xorvio-media/src/gateway/cast/`, `src-tauri/src/cast.rs`, `src/player/cast.tsx`): the player looks for `_googlecast._tcp` over mDNS once it first opens (macOS then asks about the local network), and a Cast button beside AirPlay lists the TVs found (speakers and speaker groups are left out). Choosing one connects over Cast v2 (protobuf frames over TLS on port 8009, written here: the crates about are synchronous and can't tell the receiver about fragmented-MP4 HLS), opens Google's Default Media Receiver (CC1AD845) and loads the title with its name, episode or programme, artwork (web addresses without a login only) and position. The player here pauses (a channel's stream goes: one provider connection) and shows "Playing on <TV>" with play/pause, seeking for films and episodes, the TV's volume and Stop casting, which carries on here from where the TV was; the media keys work the TV, and another channel or the next episode chosen meanwhile goes to the TV too. What the TV gets:

  | Stream | On the TV |
  | --- | --- |
  | MP4/MOV with 8-bit H.264 and AAC or MP3; WebM with VP8/VP9 and Opus/Vorbis | as it is, ranges passed through; the TV seeks itself |
  | Other films and episodes (MKV, AVI, MPEG-TS recordings, HEVC, 10-bit, Dolby/DTS audio, HLS) | ffmpeg's fragmented MP4, streamed as made: H.264 copied (else encoded, 1080p at most), AAC copied (else made AAC); seeking starts it again from there, as the player's remux does |
  | Live channels | a live buffer (as for pausing live TV) made for the TV: HLS of fragmented MP4, H.264 and AAC, five minutes kept |
  | A recording still being made, other schemes, or (without ffmpeg) anything but web files and channels | not offered: the list says why |

  HEVC and Dolby audio are converted because a sender can't ask whether the TV (or the AV receiver behind it) decodes them. The TV fetches from a second gateway listener that opens only while casting, bound to this computer's address on the TV's network on a random port, answering only the TV's addresses, and serving only the streams registered for it under a token of its own; the player's streams and everything else stay on 127.0.0.1, and no address holds a provider's. With the macOS firewall on, the first cast asks whether Xorvio may accept incoming connections; a TV that never fetched anything is reported as probably blocked by it.
- **Pause and rewind live TV** (Settings › Live buffer: off, from the moment a channel is paused, or always, keeping 15–120 minutes). A buffered channel plays as live HLS from this computer (`crates/xorvio-media/src/timeshift/`): ffmpeg writes a sliding window of 2-second fragmented-MP4 segments into `$TMPDIR/xorvio-timeshift/<session>/`, which the gateway serves (kind `l`), so pausing, seeking and the seekable range come from the player (WebKit's own HLS player on macOS, hls.js elsewhere). The provider sees one connection: for MPEG-TS and FLV the gateway reads the stream and pipes it into ffmpeg, after reading the PAT/PMT (or FLV tags) for the codecs and starting at a keyframe; for HLS, ffmpeg reads the playlist, with the codecs from its CODECS attribute or one segment. H.264, HEVC (tagged `hvc1`), AAC and AC-3/E-AC-3 are copied; MPEG-2 and other video become H.264, other audio AAC. Releasing the stream stops ffmpeg and deletes the folder. Starting takes two or three seconds (a keyframe, then the first segment); a channel copied as it is costs its own bitrate on disk (a 5 Mbit/s channel is about 38 MB a minute, 1.1 GB for 30 minutes). Recording a buffered channel still opens a connection of its own.

## Recording

- **Record Now** on a live channel records until the programme's end (plus padding), or until stopped.
- **Record** on a future guide programme (guide cell, programme sheet, watchlist card, search result) schedules it. Default padding is 1 minute before and 3 after (Settings › Recording).
- The recorder runs `ffmpeg -c copy` into MPEG-TS (robust against a cut-off end), reconnecting through stream drops until the end time. A recording that lost time is marked *Partial*, never thrown away.
- Recordings are a sidebar screen (Scheduled, Recording, Recorded) and play in the same player, with Continue Watching progress.
- Scheduled recordings need Xorvio running. While any are pending, closing the window keeps Xorvio in the menu bar (tray), and it keeps the computer awake while recording.
- Watching and recording count as provider connections. Before a recording starts, the app warns when it would exceed the provider's reported `max_connections`.
- **Record Series** (programme sheet, guide right-click) makes a series recording (`recorder/rules.rs`): the programme's title, matched the way the watchlist matches titles, on this channel or any channel; new episodes only (the default) or every airing; keep all or the latest 3, 5 or 10; padding from Settings or its own. Series look in the guide (`Core::airings`, `crates/xorvio-core/src/airings.rs`) after every guide refresh and every 15 minutes, up to 14 days ahead, and schedule each airing once. New episodes only skips airings XMLTV marks as previously shown (unless it also says new) and episodes the series already has, by season and episode number, else subtitle, else a lone episode number, else description (a description match doesn't stop an airing the guide calls new). With any channel, an airing on several versions of one channel (HD, FHD, 4K, grouped as Live TV groups them) records once, from the version that plays. A cancelled or deleted airing isn't scheduled again; a failed one, or a partial one that caught less than half, is recorded at a later airing. A series' history (`series_history`) remembers episodes recorded and airings cancelled after their recordings are gone. Keeping the latest few moves older finished ones to the Trash (the `trash` crate; NSFileManager on macOS, so no Finder prompt; deleted where there's no Trash). A programme the guide moves is rescheduled at its new time. Changing a series reschedules its airings that haven't started.
- **Conflicts**: scheduled and running recordings from one source that need more streams at once than its `max_connections` are marked (`Recording.conflict`, worked out when listed) and shown on the recording and its series. Series schedule them anyway; recording by hand still warns first.
- **Save from Catch-up** on a programme that aired, on a channel with catch-up (where Play Replay is offered), saves the provider's replay (`recorder/catchup.rs`): ffmpeg reads it as fast as it comes for the programme's length, with as much padding as the archive still holds, into MPEG-TS; a replay that breaks off is asked for again from where it stopped, three times at most, and the parts are joined. It holds a provider stream while it runs, and the same warning applies. It carries on after a restart.
- **Folders** (Settings › Recording): the recordings folder (Movies › Xorvio unless chosen) and a folder per source. A recording's folder is chosen when it starts: its source's, else the recordings folder, else Movies › Xorvio. A chosen folder that's missing or can't be written (an unplugged drive) is passed over, never made, and the recording says where it went (`Recording.note`).
- **Ad breaks** (`crates/xorvio-media/src/adbreaks/`, `recorder/ads.rs`; Settings › Recording › Ad breaks: Off, Show a Skip button (the default), Skip automatically). When a recording finishes (unless Off), or when Find Ad Breaks is chosen in its menu, its breaks are looked for in the background, one recording at a time at a low priority, carrying on after a restart; the player marks them on the seek bar and offers Skip Ad in each (or skips each once, and says so). With comskip installed (`PATH`, `/opt/homebrew/bin`, `/usr/local/bin`; a separate GPL program, never bundled) its EDL is used, written in a temporary folder of its own; otherwise, or when it fails, one ffmpeg pass with `blackdetect` (≥ 0.08 s, 98% of the picture dark) and `silencedetect` (≥ 0.08 s below −50 dB) feeds strict rules (`adbreaks/rules.rs`), since a missed break costs a press of the remote and a skipped stretch costs the programme:
  - a *join* is black overlapping silence (within ¼ s, at most 5 s long; joins within a second are one);
  - a *spot* is the stretch between two joins, middle to middle, within a second of 10, 15, 20, 30 or 60 s;
  - a *break* is three or more spots back to back adding up to 85 s – 6¼ min, from its first join's black to the end of its last one's.

  Scene changes (`scdet`) aren't used: they come at every cut, adding joins in the programme without finding spots black and silence miss. On synthetic broadcasts made with ffmpeg (tests/adbreaks.rs, at 10 and 25 fps, joins of 3–6 frames, spots a frame or two off), every break is found within a second, and a programme of fades, a dark shot, a hush, two spot-length stretches in a row and three short spots gives none. The recording file is only read.
- `recordings.sqlite` migrates in place (`PRAGMA user_version`, one transaction per step, only adding): version 1 added `rule_id`, `episode`, `episode_key`, `airing_key`, `from_catchup` and `note` to `recordings`, and the `series_rules` and `series_history` tables; version 2 added `ad_scan` (NULL, `queued`, `found` or `failed`), `ad_breaks` (JSON) and `ad_scan_note` (who found them, or why they couldn't be found).

## Packaging

- **ffmpeg comes with the app on every platform.** `Scripts/build-ffmpeg.sh` builds ffmpeg and ffprobe from pinned, checksummed upstream releases (FFmpeg 9.0.2, x264's stable branch, dav1d 1.5.4, and OpenSSL 3.5 on Linux) into `build/ffmpeg/`, then copies them to `src-tauri/binaries/xorvio-{ffmpeg,ffprobe}-<target triple>`, both gitignored. `src-tauri/tauri.ffmpeg.conf.json` adds them as `bundle.externalBin`, and Tauri puts them next to the app's executable without the suffix (`Contents/MacOS`, the Windows install folder, `/usr/bin` in the `.deb`, `usr/bin` in the AppImage) and signs them on macOS. The `xorvio-` names keep the `.deb`'s copies from clashing with the distribution's `/usr/bin/ffmpeg`. `npm run app` stays as it was; `Scripts/build-app.sh --with-ffmpeg` adds them.
- **Why Linux bundles it too:** the distributions' ffmpeg is too old for what Xorvio passes it. Ubuntu 22.04 has 4.4 and 24.04 has 6.1, and neither's HLS demuxer takes `-allowed_segment_extensions` or `-extension_picky`, which the live buffer gives it for HLS channels (22.04's lacks `-seg_max_retry` too); Debian 13's 7.1 does, though it seeks copied video a keyframe earlier than 9 does.
- **`Tools::locate()`** takes the folder in `XORVIO_FFMPEG_DIR` and nothing else when that's set (for running the tests against a build: `XORVIO_FFMPEG_DIR=$PWD/src-tauri/binaries cargo test -p xorvio-media`; the tests then make their test media with the ffmpeg installed on the computer). Otherwise: Xorvio's own next to the executable (`xorvio-ffmpeg`, with or without the triple), any `ffmpeg` there, then `PATH`, then `/opt/homebrew/bin` and `/usr/local/bin`. On Windows ffmpeg starts without a console window, and on Linux with `SSL_CERT_FILE` pointing at the distribution's certificates when they aren't where the bundled OpenSSL looks (`process::tool_command`).
- **Only what Xorvio uses is built in**, listed in the script with where each part is used: http(s), tcp, tls, crypto (HLS keys), file and pipe; the containers sources come in, HLS and concat; decoders for everything the remux may have to convert, including VP9, AV1 (dav1d; ffmpeg's own AV1 decoder needs a hardware decoder), and PGS, VobSub and DVB pictures for drawing subtitles in; libx264, AAC and WebVTT encoders; MPEG-TS, HLS, MP4 and WebVTT muxers; the yadif, scale, overlay, null, aformat and aresample filters; and the bitstream filters copying needs. zlib is in for Matroska's compressed tracks (mkvmerge compresses PGS) and gzip from servers. Everything is linked statically except system libraries, so each binary stands alone (about 11 MB each on Apple silicon). `-buildconf` keeps no path of the computer that built it.
- **TLS.** ffmpeg is given https addresses itself: films and files it remuxes, HLS playlists the live buffer reads, and channels it records. It checks certificates (ffmpeg 9's default). macOS and Windows builds use the system's TLS and trust store: Secure Transport (TLS 1.2 at most, deprecated by Apple but still there; OpenSSL would bring TLS 1.3 if a provider ever needs it) and Schannel. Linux has no system TLS ffmpeg can use, so its build links OpenSSL 3.5 LTS in, pinned, reading the distribution's certificates.
- **Licence.** With x264 each binary is GPL-2.0-or-later (GPL-3.0-or-later on Linux, with OpenSSL), which Xorvio's GPL-3.0-or-later covers. `THIRD-PARTY.md` lists what's in them, and every release carries their exact sources and build notes.
- **macOS signing.** `npm run app` signs with the Apple Development certificate as before. `Scripts/build-app.sh --release` signs with a Developer ID Application certificate and the hardened runtime (Tauri's default, for the app and the sidecars alike), notarizes with `notarytool`, staples the app and a disk image, and checks both with Gatekeeper. No entitlements are needed: WKWebView's JavaScript JIT runs in WebKit's own processes, which carry their own, and Xorvio isn't sandboxed, so the network and files need none. The README's "Releases" says what to set up.
- **CI** (`.github/workflows/ci.yml`) runs clippy (warnings shown, not yet fatal), the tests against each platform's bundled ffmpeg and the TypeScript check on macOS, Ubuntu 22.04 and Windows. **Releases** (`release.yml`) build on a `v*` tag with `tauri-apps/tauri-action` into a draft release: macOS disk images for Apple silicon and Intel (both built on Apple silicon; signed and notarized when the Apple secrets are set, otherwise ad hoc), Windows NSIS and MSI installers, and a Linux AppImage and `.deb` built on Ubuntu 22.04. Windows' ffmpeg is built in MSYS2 by the same script rather than taken from a third party's build: the same parts and licences as elsewhere, sources that are exactly upstream's, and nothing to trust but upstream. `.github/actions/xorvio-ffmpeg` builds it for both workflows and caches it until the script changes.

## The contract

Types are in `crates/xorvio-model/src/lib.rs` (Rust) and `src/lib/types.ts` (TypeScript), JSON in camelCase, times in Unix seconds. The Tauri commands in `src-tauri/src/commands.rs` are the only way the interface reaches the backend; `src/lib/api.ts` wraps them, and `src/lib/mock.ts` stands in for them in a plain browser (the demo library), so screens can be built and checked with `npm run dev` alone.

### xorvio-core

```rust
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> anyhow::Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> anyhow::Result<()>;
    fn delete(&self, key: &str) -> anyhow::Result<()>;
}
pub struct KeyringSecrets;            // the OS keychain, service "com.xorvio.Xorvio"
pub struct MemorySecrets;             // tests

pub struct Core;                      // Send + Sync, shared as Arc<Core>
impl Core {
    pub fn open(data_dir: &Path, secrets: Arc<dyn SecretStore>) -> Result<Arc<Core>>;
    pub fn set_tidy_names(&self, tidy: bool);          // "UK | Channel One FHD" → "Channel One" for channels and categories (default on)
    pub fn set_group_similar_channels(&self, group: bool); // "News 24 HD", "US: News 24 4K" → one row in `channels` (default on; ChannelFilter::group_similar overrides)
    pub fn set_tvmaze_enabled(&self, enabled: bool);   // TVmaze fills series gaps, title only sent (default on)

    // Sources
    pub fn list_sources(&self) -> Result<Vec<SourceSummary>>;
    pub async fn add_source(&self, new: NewSource) -> Result<SourceConfig>;   // checks the login/playlist, saves; no refresh
    pub fn update_source(&self, config: SourceConfig, secrets: Option<SourceSecrets>) -> Result<()>;
    pub fn remove_source(&self, id: &str) -> Result<()>;
    pub fn source_secrets(&self, id: &str) -> Result<SourceSecrets>;
    pub fn add_demo_source(&self) -> Result<()>;
    pub async fn refresh_source(&self, id: &str, progress: &(dyn Fn(RefreshProgress) + Send + Sync)) -> Result<SourceStatus>; // catalog, then guide
    pub async fn refresh_guide(&self, id: &str) -> Result<SourceStatus>;
    pub fn cancel_refresh(&self, id: &str) -> bool;                     // heard while downloading and between steps, never mid-write: the catalog (or the guide) stays as it was
    pub fn sources_due_for_refresh(&self, now: Timestamp) -> Result<Vec<SourceId>>;
    pub fn reorder_sources(&self, ids: &[String]) -> Result<()>;        // the order Live TV lists channels in across sources
    pub async fn test_xtream_login(&self, secrets: &SourceSecrets, user_agent: Option<&str>) -> Result<XtreamAccountCheck>; // Test Connection: connections and expiry, never the login
    pub fn keep_playlist_copy(&self, file_name: &str, contents: &[u8]) -> Result<String>; // a playlist dropped on the add sheet, kept in Playlists/ until its source goes
    // xtream_login_in_link(link) -> Option<SourceSecrets>: the Xtream login in a get.php playlist link, for the sheet's "Use Xtream?"

    // Catalog
    pub fn categories(&self, kind: CategoryKind) -> Result<Vec<Category>>;
    pub fn channels(&self, filter: &ChannelFilter) -> Result<Vec<Channel>>;
    pub fn channel(&self, id: &str) -> Result<Option<Channel>>;
    pub fn movies(&self, query: &BrowseQuery) -> Result<Vec<Movie>>;
    pub fn title_marks(&self, kind: CategoryKind, ids: &[String]) -> Result<Vec<TitleMark>>; // a page's version counts, watched and progress
    pub fn movie_versions(&self, movie_id: &str) -> Result<Vec<TitleVersion>>; // every copy of a film, for its Versions picker
    pub async fn fill_posters(&self, ids: &[String]) -> Result<Vec<(String, String)>>; // TMDB posters for titles shown without one, kept
    pub fn movie(&self, id: &str) -> Result<Option<Movie>>;
    pub fn series_list(&self, query: &BrowseQuery) -> Result<Vec<Series>>;
    pub fn series(&self, id: &str) -> Result<Option<Series>>;
    pub async fn episodes(&self, series_id: &str) -> Result<Vec<Episode>>;   // Xtream get_series_info on first ask, then cached; TMDB fills stills, synopses and numbered-only names
    pub fn episode(&self, id: &str) -> Result<Option<Episode>>;
    pub async fn details(&self, item_id: &str) -> Result<MediaDetails>;      // provider detail calls (once per title), TMDB over them (under a media server's), TVmaze for series gaps TMDB can't fill
    pub async fn related_movies(&self, movie_id: &str) -> Result<Vec<Movie>>;  // TMDB's recommendations the sources carry, each once; none without a key
    pub async fn related_series(&self, series_id: &str) -> Result<Vec<Series>>;
    pub async fn person(&self, name: &str, tmdb_id: Option<&str>, item_id: Option<&str>) -> Result<Option<PersonDetails>>; // TMDB's page for someone in a cast (Wikidata's filmography without one), with their titles here
    pub fn tmdb_status(&self) -> TmdbStatus;
    pub async fn tmdb_set_key(&self, key: &str) -> Result<TmdbStatus>;     // checked with TMDB, then kept in the keychain; empty removes it
    pub fn genres(&self, kind: CategoryKind) -> Result<Vec<String>>;
    pub fn top_titles(&self, kind: CategoryKind, limit: usize) -> Result<(Vec<Movie>, Vec<Series>)>; // this year's titles carried most
    pub fn search(&self, text: &str, limit: usize) -> Result<SearchResults>; // FTS5 prefix search, programmes included
    pub fn search_with(&self, text: &str, limit: usize, program_limit: usize) -> Result<SearchResults>; // more programmes, for Show All Listings

    // Guide
    pub fn guide(&self, channel_ids: &[String], from: Timestamp, to: Timestamp) -> Result<Vec<GuideRow>>;
    pub fn now_next(&self, channel_ids: &[String], now: Timestamp) -> Result<Vec<GuideRow>>; // up to 2 programmes each
    pub fn program(&self, program_id: &str) -> Result<Option<ProgramHit>>;
    pub fn upcoming_programs(&self, from: Timestamp, to: Timestamp, limit: usize) -> Result<Vec<ProgramHit>>; // "Tonight on TV": films, sport, live/new/premiere and artwork first; no news or paid programming; repeats last; ≤ 2 per channel; by start time
    pub fn search_guide(&self, text: &str, from: Timestamp, limit: usize) -> Result<Vec<ProgramHit>>; // titles containing text, ending after from: still to come first, then what aired, latest first
    pub fn guide_span(&self) -> Result<Option<(Timestamp, Timestamp)>>;
    pub fn guide_listing_count(&self) -> Result<i64>;                         // Settings › Storage
    pub fn clear_guide_listings(&self) -> Result<()>;                        // every listing, then VACUUM (slow for a big guide: off the main thread); refreshes bring them back

    // Added guides (XMLTV beside the sources' own; link in the keychain) and guide channels chosen by hand
    pub fn guide_sources(&self) -> Result<Vec<GuideSourceSummary>>;
    pub fn add_guide_source(&self, new: NewGuideSource) -> Result<GuideSource>;           // saves; no download
    pub fn update_guide_source(&self, guide: GuideSource, url: Option<String>) -> Result<()>;
    pub fn remove_guide_source(&self, id: &str) -> Result<()>;
    pub fn guide_source_link(&self, id: &str) -> Result<Option<String>>;
    pub async fn refresh_guide_source(&self, id: &str) -> Result<SourceStatus>;
    pub fn guides_due_for_refresh(&self, now: Timestamp) -> Result<Vec<String>>;        // daily; a failed one waits an hour
    pub fn guide_channels(&self, text: &str, limit: usize) -> Result<Vec<GuideChannel>>; // every guide's channels by id or name
    pub fn guide_match(&self, channel_id: &str) -> Result<GuideMatch>;
    pub fn match_guide(&self, channel_id: &str, guide_id: &str, epg_id: &str) -> Result<()>; // kept in the library, wins through refreshes
    pub fn reset_guide_match(&self, channel_id: &str) -> Result<()>;

    // Library
    pub fn watchlist(&self) -> Result<Vec<WatchlistEntry>>;                // Home order (Watchlist::ordered)
    pub fn set_watchlisted(&self, entry: WatchlistEntry, saved: bool) -> Result<()>;
    pub fn favorite_channel_ids(&self) -> Result<Vec<String>>;
    pub fn set_favorite(&self, channel_id: &str, favorite: bool) -> Result<()>;
    pub fn save_progress(&self, progress: PlaybackProgress) -> Result<()>;
    pub fn progress(&self, item_id: &str) -> Result<Option<PlaybackProgress>>;
    pub fn continue_watching(&self, limit: usize) -> Result<Vec<ContinueItem>>; // in progress + next episode up
    pub fn record_channel_watched(&self, channel_id: &str) -> Result<()>;
    pub fn recent_channels(&self, limit: usize) -> Result<Vec<Channel>>;
    pub fn due_reminders(&self, now: Timestamp, lead_seconds: i64) -> Result<Vec<Reminder>>; // reminded programmes starting within the lead (or ≤ 2 min ago), not yet sent
    pub fn mark_reminders_sent(&self, item_ids: &[String], now: Timestamp) -> Result<()>;  // kept in library.sqlite, so a relaunch doesn't repeat them
    pub fn new_episodes_due(&self, now: Timestamp) -> Result<bool>;                // an hour since the last check
    pub fn followed_addon_series(&self, now: Timestamp) -> Result<Vec<String>>;   // followed add-on series due a look, for the app to fetch
    pub async fn check_new_episodes(&self, now: Timestamp, addon_shows: Vec<(Series, Vec<Episode>)>) -> Result<Vec<NewEpisodes>>; // watchlist series and caught-up ones; the first look only notes where each is
    pub fn new_episode_counts(&self) -> Result<HashMap<String, i64>>;              // "New Episode" badges by series
    pub fn mark_episodes_seen(&self, series_id: &str) -> Result<bool>;             // opening a series clears its badge
    pub fn new_releases(&self, limit: usize) -> Result<Vec<Movie>>;                // this year's and last year's films, one card each
    pub fn pending_addon_series(&self) -> Result<Vec<String>>;                     // add-on series in New Episodes, whose episodes the app supplies
    pub fn new_episode_series(&self, limit: usize, addon_shows: &HashMap<String, (Series, Vec<Episode>)>) -> Result<Vec<Series>>; // followed series with new episodes not watched yet, then followed ones whose next episode aired in the last two weeks

    // Channel groups (named favourites lists; "default" is Favorites)
    pub fn favorite_lists(&self) -> Result<Vec<FavoriteList>>;
    pub fn create_favorite_list(&self, name: &str, channel_id: Option<&str>) -> Result<FavoriteList>;
    pub fn rename_favorite_list(&self, id: &str, name: &str) -> Result<()>;
    pub fn delete_favorite_list(&self, id: &str) -> Result<()>;
    pub fn set_in_favorite_list(&self, list_id: &str, channel_id: &str, member: bool) -> Result<()>;
    pub fn reorder_favorite_list(&self, list_id: &str, channel_ids: &[String]) -> Result<()>; // channels left out keep their places

    // Names, hiding and the parental PIN (channels and categories; lists leave out hidden and locked ones, lookups by id don't)
    pub fn rename_item(&self, item_id: &str, name: Option<&str>) -> Result<()>;   // None goes back to the provider's name
    pub fn set_hidden(&self, item_id: &str, hidden: bool) -> Result<()>;        // films and series too ("Hide from Library")
    pub fn show_all_hidden(&self) -> Result<()>;
    pub fn set_locked(&self, item_id: &str, locked: bool) -> Result<()>;         // unlocking needs the PIN entered
    pub fn set_catchup_offset(&self, channel_id: &str, minutes: i64) -> Result<()>; // −30…30 (0 none): replays ask for start and end moved by it
    pub fn set_preferred_version(&self, channel_id: &str, preferred: bool) -> Result<()>; // "Always play this version" for its group; false: the best again
    pub fn item_edits(&self) -> Result<Vec<ItemEdit>>;                            // for Settings and menus
    pub fn categories_with_hidden(&self, kind: CategoryKind) -> Result<Vec<Category>>;
    pub fn parental_status(&self) -> Result<ParentalStatus>;
    pub fn set_pin(&self, current_pin: Option<&str>, new_pin: Option<&str>) -> Result<()>; // salted hash in library.sqlite; adult-looking categories lock themselves
    pub fn unlock(&self, pin: &str) -> Result<bool>;                              // until locked, the time below passes, or quit; five wrong tries wait a minute
    pub fn lock(&self);
    pub fn set_relock_after(&self, after: Option<Duration>);                      // Settings' "Lock again after" (5 min…4 hours; None: when Xorvio quits)

    // Library profiles and Home (library.sqlite; the active profile applies to every list through `excluded`)
    pub fn profiles(&self) -> Result<Profiles>;                                  // in the order made, with the active id; Everything alone until another is made
    pub fn set_active_profile(&self, id: &str) -> Result<()>;
    pub fn save_profile(&self, profile: LibraryProfile) -> Result<LibraryProfile>; // no id: a new one; names are kept apart
    pub fn delete_profile(&self, id: &str) -> Result<()>;                        // with its Home layouts; never the last one
    pub fn profile_categories(&self) -> Result<Vec<ProfileCategory>>;            // every category with its languages, genres, adult flag and count (locked ones wait for the PIN)
    pub fn home_layout(&self, mode: &str) -> Result<Option<serde_json::Value>>;  // "sources" or "addons"; None: the standard layout
    pub fn save_home_layout(&self, mode: &str, layout: Option<serde_json::Value>) -> Result<()>;
    pub fn on_now(&self, limit: usize) -> Result<Vec<ProgramHit>>;               // favourite and recent channels first, then others airing
    pub fn watchlist_shown(&self) -> Result<Vec<WatchlistEntry>>;                // the watchlist less what's hidden, locked or left out by the profile
    pub fn addon_titles_in_profile(&self, movies: &mut Vec<Movie>, series: &mut Vec<Series>) -> Result<()>; // kinds, genre fields, country tags

    // Playback
    pub fn stream_target(&self, source_id: &str, locator: &StreamLocator, is_live: bool) -> Result<StreamTarget>;
    pub fn replay_target(&self, channel_id: &str, start: Timestamp, end: Timestamp) -> Result<StreamTarget>;

    // Trakt (the tokens, the client secret and the device code stay in the backend and the keychain)
    pub fn set_trakt_listener(&self, listener: TraktListener);              // TraktEvent::Status(TraktStatus) | LibraryChanged, for `trakt-changed` and `library-changed`
    pub fn trakt_status(&self) -> TraktStatus;
    pub async fn trakt_start_sign_in(&self) -> Result<TraktSignInStart>;     // POST auth.trakt.tv/oauth/device/code
    pub async fn trakt_check_sign_in(&self) -> Result<bool>;                 // false while waiting; true once allowed (then sync)
    pub async fn trakt_disconnect(&self) -> Result<()>;                      // revokes, forgets the tokens and what Trakt said
    pub async fn trakt_sync(&self) -> Result<TraktStatus>;                   // one at a time; a failure is the status's lastError
    pub fn trakt_set_options(&self, options: TraktOptions) -> Result<()>;   // library preference; all on by default
    pub async fn trakt_set_app(&self, client_id: &str, redirect_uri: &str) -> Result<()>; // "" goes back to the built-in app; "" redirect is http://localhost
    pub async fn trakt_scrobble(&self, item_id: &str, kind: MediaKind, action: TraktScrobbleAction, progress: f64); // never fails
}

pub struct ChannelFilter { pub category_id: Option<String>, pub favorites_only: bool, pub list_id: Option<String>, pub source_id: Option<String>, pub include_hidden: bool, pub limit: Option<usize>, pub group_similar: Option<bool> }
pub struct BrowseQuery { pub category_id: Option<String>, pub genre: Option<String>, pub content_genre: Option<ContentGenre>, pub sort: BrowseSort, pub limit: usize, pub offset: usize, pub source_id: Option<String>, pub watched: WatchedFilter, pub group_versions: bool } // content_genre: genre field or a category named for it; group_versions: a film's copies as one
pub enum WatchedFilter { All, Unwatched, InProgress }
pub enum BrowseSort { RecentlyAdded, Title, Year, Rating }
```

Which guide channel a channel shows is worked out whenever a catalog or guide loads: a guide channel chosen by hand wins; then the channel's own guide id in its source's guide, then in the added guides in their order; then the same by tidy name. Each guide (a source's, via `SourceConfig::guide_offset_minutes`, or an added one) can move its programmes by an offset, applied as the guide loads.

Channels a provider lists several times are grouped in `channels` (grouping.rs): names are read by `names::version_name` (country and quality prefixes, quality and codec tags, superscripts such as ᴴᴰ, backup and alt marks; "+1" stays apart), and versions of one name within one category become one row, in the place of the first, that plays the chosen version or else the best (4K/UHD, FHD/1080, HD/720, untagged, SD; a main feed before a backup; then the first listed). The row is that version's `Channel` with `versions` listing them all. Grouping is only a way of listing: favourites, groups, history, recordings, hiding and locks keep real channel ids, and the interface applies a row's actions to its versions. The version chosen is kept in library.sqlite by the group's key (category, or source, and the folded name). Programmes carry a `genre` read from their XMLTV category in several languages (genres.rs) for the guide's colours; it isn't stored.

### xorvio-addons

```rust
pub struct Addons;                    // Arc<Addons>
impl Addons {
    pub fn open(data_dir: &Path, secrets: Arc<dyn SecretStore>) -> Result<Arc<Addons>>; // SecretStore re-declared here (same shape)
    pub fn list(&self) -> Vec<AddonInfo>;
    pub async fn install(&self, manifest_url: &str) -> Result<AddonInfo>;
    pub fn remove(&self, id: &str) -> Result<()>;
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()>;
    pub fn move_addon(&self, id: &str, to_index: usize) -> Result<()>;
    pub async fn discover_rows(&self) -> Result<Vec<AddonCatalogRow>>;   // first page of every browsable catalog
    pub async fn catalog(&self, addon_id: &str, media_type: &str, catalog_id: &str, genre: Option<&str>, skip: usize) -> Result<AddonCatalogRow>;
    pub async fn search(&self, text: &str) -> Result<(Vec<Movie>, Vec<Series>)>;
    pub async fn meta(&self, media_type: &str, media_id: &str) -> Result<AddonMeta>;
    pub async fn streams(&self, media_type: &str, media_id: &str) -> Result<Vec<AddonStream>>;
    pub async fn subtitles(&self, media_type: &str, media_id: &str) -> Result<Vec<AddonSubtitle>>;
}
```

Add-on titles map onto the shared `Movie`/`Series`/`Episode` types with `source_id = "addon"`, ids `addon|<type>|<meta id>` and `StreamLocator::Addon`.

### xorvio-media

```rust
pub struct Tools { pub ffmpeg: PathBuf, pub ffprobe: PathBuf }   // Tools::locate(): XORVIO_FFMPEG_DIR alone if set; else bundled sidecar, PATH, Homebrew
pub struct Gateway;                   // Arc<Gateway>
impl Gateway {
    pub async fn start(tools: Option<Tools>) -> Result<Arc<Gateway>>;     // binds 127.0.0.1:0
    pub async fn prepare(&self, target: StreamTarget, hint: PlayHint) -> Result<ResolvedStream>;
    pub async fn seek(&self, stream_url: &str, offset_seconds: f64) -> Result<ResolvedStream>; // remuxed streams
    pub fn release(&self, stream_url: &str);
    pub fn external(&self, stream_url: &str, title: &str) -> Result<String>; // for another player: the original stream, kept until quit (16 at most)
    pub fn external_target(&self, target: StreamTarget, hint: &PlayHint, title: &str) -> Result<String>; // the same for one not playing
    pub async fn timeshift(&self, target: StreamTarget, hint: PlayHint, window_seconds: f64) -> Result<ResolvedStream>; // a live channel from a buffer; returns once the first segment is written
    pub async fn timeshift_playing(&self, stream_url: &str, window_seconds: f64) -> Result<ResolvedStream>; // the live channel playing directly, to a buffer from now (released first: one connection)

    // Casting (gateway/cast): a listener on the network, only while casting, for the TV alone
    pub fn cast_check(&self, target: &StreamTarget, hint: &PlayHint) -> Option<String>; // why it can't go to a TV, said before anything opens
    pub async fn cast(&self, target: StreamTarget, hint: PlayHint, start_seconds: f64, tv: &[IpAddr]) -> Result<CastMedia>; // what the TV fetches; what was cast before stops
    pub async fn cast_seek(&self, url: &str, offset_seconds: f64) -> Result<CastMedia>; // ffmpeg's stream again from there
    pub fn cast_requests(&self) -> usize;                                     // none: the TV couldn't reach this computer
    pub fn cast_end(&self);                                                   // stops it and closes the listener

    // Seek-bar thumbnails (gateway/thumbnails.rs), served as kind `p`
    pub fn set_thumbnail_cache(&self, dir: PathBuf, limit_bytes: u64);        // least recently used go first
    pub fn thumbnails(&self, stream_url: &str, item_id: &str, may_connect: ConnectionCheck) -> Thumbnails; // Ready { sheet } | Making | Unavailable { reason }; the first ask starts them
}
pub type ConnectionCheck = Arc<dyn Fn(&str /* the title's host */) -> Result<(), String> + Send + Sync>; // asked before a remote title's every tile
pub fn second_connection(allowance: Allowance, recordings: usize) -> Result<(), String>; // Allowance::{Unlimited, Streams(n), Unknown}: only Unlimited or n > 1, with no recording
pub struct PlayHint { pub extension: Option<String>, pub live_format: Option<LiveStreamFormat>, pub duration_seconds: Option<f64> }

pub trait ChannelResolver: Send + Sync {
    fn resolve(&self, source_id: &str, channel_id: &str) -> Result<StreamTarget>;
}
pub struct Recorder;                  // Arc<Recorder>
impl Recorder {
    pub fn open(data_dir: &Path, recordings_dir: PathBuf, tools: Option<Tools>, resolver: Arc<dyn ChannelResolver>) -> Result<Arc<Recorder>>;
    pub fn start(self: &Arc<Self>, on_change: Arc<dyn Fn(Recording) + Send + Sync>); // the scheduler loop (tokio)
    pub fn list(&self) -> Result<Vec<Recording>>;
    pub fn schedule(&self, request: ScheduleRequest) -> Result<Recording>;   // starts at once if already due
    pub fn record_now(&self, request: ScheduleRequest) -> Result<Recording>;
    pub fn stop(&self, id: &str) -> Result<()>;                              // keeps what was recorded
    pub fn cancel(&self, id: &str) -> Result<()>;                            // a scheduled one
    pub fn delete(&self, id: &str, delete_file: bool) -> Result<()>;
    pub fn set_recordings_dir(&self, dir: PathBuf) -> Result<()>;
    pub fn set_folders(&self, folders: RecordingFolders);                   // per source, with the fallback
    pub fn active_count(&self) -> usize;
    pub fn record_catchup(&self, request: ScheduleRequest) -> Result<Recording>; // a programme that aired
    pub fn streams_in_use(&self, source_id: &str, from: Timestamp, to: Timestamp) -> Result<usize>;
    // Series recordings (rules.rs)
    pub fn set_guide(&self, guide: Arc<dyn SeriesGuide>);
    pub fn set_default_padding(&self, before_seconds: i64, after_seconds: i64);
    pub fn on_list_change(&self, callback: Arc<dyn Fn() + Send + Sync>);   // "recordings-changed"
    pub fn rules(&self) -> Result<Vec<SeriesRule>>;
    pub fn add_rule(&self, new: NewSeriesRule, chosen: Option<Airing>) -> Result<SeriesRule>;
    pub fn update_rule(&self, id: &str, options: SeriesRuleOptions) -> Result<SeriesRule>;
    pub fn delete_rule(&self, id: &str, delete_recordings: bool) -> Result<()>;
    pub fn evaluate_rules(&self) -> Result<usize>;                           // blocks; evaluate_rules_soon() doesn't
    // Ad breaks (ads.rs, adbreaks/)
    pub fn set_find_ad_breaks(&self, automatic: bool);                     // look through recordings as they finish
    pub fn ad_breaks(&self, id: &str) -> Result<AdBreaks>;                   // { state: Never | Finding | Found | Failed, breaks, found_by, error }
    pub fn find_ad_breaks(&self, id: &str) -> Result<AdBreaks>;              // now, ahead of any waiting
}
// ChannelResolver also has replay(source, channel, start, end) and max_connections(source), with defaults.
```

## Status

| Area | State |
| --- | --- |
| Workspace, model, docs | Done |
| Core: sources, stores, search (69 tests) | Done |
| Add-ons (Stremio protocol) | Done |
| Gateway and recorder (34 tests) | Done |
| Tauri shell, commands, tray, keep-awake, Swift import | Done |
| Interface: shell, Home, Live TV + guide keys, Films, Series, title pages, player, Watchlist, Recordings, Search, Sources, Settings, Discover | Done (first pass) |
| End to end (`cargo test -p xorvio --test end_to_end -- --ignored`) | Passes: demo channel through the gateway, a 20 s recording, playback of it |
| Trakt: connecting, scrobbling, history, places and the watchlist both ways | Done; connected to a real Trakt app |
| TMDB details with the person's own key, and plain-http artwork through the app | Done; tried with a real key and the user's provider |
| Channel management: renaming, hiding, locking, channel groups, the parental PIN | Done |
| Reminder notifications (Settings › Reminders; a 30 s loop in the app, so they go out from the menu bar too; clicking one tunes the channel) | Done; not yet tried in the installed app |
| Series recordings, conflicts, saving from catch-up, recording folders per source | Done; tested against a stand-in guide and catch-up server, not yet tried in the installed app |
| Multiview, pop-out player, library profiles and a customizable Home, media folders, diagnostics, Viewing History | Done; profiles, diagnostics and history tried in the installed app, Multiview and pop-out not yet with playback |
| Chromecast and Google TV: discovery, Cast v2, the TV's listener, the casting state | Done; tested against a fake receiver, not yet with a real TV |
| VP9/AV1: encoded when remuxed, played directly where the web view decodes them | Done |
| Packaging: Xorvio's own ffmpeg (all platforms), Developer ID signing and notarization, CI and release workflows | Done; ffmpeg built and the media tests passed against it on macOS and Linux, Windows and the workflows not yet run, no Developer ID yet |
| Backup (by hand, with sealed logins, and automatic), restore across a restart, sync through a shared folder | Done; not yet tried in the installed app |
| Thumbnails while scrubbing; ad breaks found in recordings, marked and skipped | Done; tested with generated media and a stand-in provider, not yet tried in the installed app |
| Picture subtitles drawn into the video; diagnostics; Viewing History; live TV latency | Done; diagnostics and history tried in the installed app |
| Parity with the Swift app: Home and watchlist (new rows, live-only Home, banners, keys, new-episode alerts, watchlist controls, reminders), browse and search (filters, versions, ratings, videos, hiding, categories, Wikidata people, posters from TMDB), Sources and Settings (schedules, order, cancelling, Test Connection, tidy names, relocking), the menu bar and first run, the player (reconnecting, channel list, numbers, Now/Next, keys, zap delay, track memory, copies, docked and mini controls) | Done; the menu bar and profiles tried in the installed app, the rest by tests and the browser preview |
| Still apart from the Swift app | Reminders after Xorvio quits (macOS scheduled notifications), tidied channel logos, the Personal add-on options (a quality cap, filling missing episodes), Support and Privacy links once the project has a site |
