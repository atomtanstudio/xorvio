//! A media folder's library page and the changes made there, against
//! temporary folders of files made here. Nothing reads or changes anyone's
//! own folders.

use std::path::Path;
use std::sync::Arc;

use xorvio_model::{ClipKind, ClipLength, FileFacts, FolderQuery, FolderSort, MediaKind, PlaybackProgress, RefreshProgress};

use super::*;
use crate::MemorySecrets;

fn make(root: &Path, paths: &[&str]) {
    for path in paths {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, path.to_string_lossy().as_bytes()).unwrap();
    }
}

fn ignore(_: RefreshProgress) {}

struct Library {
    _data: tempfile::TempDir,
    _folder: tempfile::TempDir,
    core: Arc<Core>,
    source: String,
    root: PathBuf,
}

impl Library {
    async fn new(paths: &[&str]) -> Self {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("Videos");
        make(&root, paths);
        let core = Core::open(data.path(), Arc::new(MemorySecrets::new())).unwrap();
        let source = core.add_folder_source("", &[root.to_string_lossy().into_owned()]).unwrap().id;
        core.refresh_source(&source, &ignore).await.unwrap();
        Self { _data: data, _folder: folder, core, source, root }
    }

    fn query(&self, folder: Option<&str>) -> FolderQuery {
        FolderQuery {
            source_id: self.source.clone(),
            folder: folder.map(|folder| self.path(folder)),
            below: false,
            kind: None,
            length: None,
            sort: FolderSort::Name,
            limit: 100,
            offset: 0,
        }
    }

    fn path(&self, below: &str) -> String {
        if below.is_empty() { self.root.to_string_lossy().into_owned() } else { self.root.join(below).to_string_lossy().into_owned() }
    }

    fn listing(&self, query: &FolderQuery) -> FolderListing {
        self.core.folder_listing(query).unwrap()
    }

    /// The paths of every video the catalog has, below the folder.
    fn all(&self) -> Vec<String> {
        let mut paths: Vec<String> = self.listing(&FolderQuery { below: true, ..self.query(None) }).videos.into_iter().map(|video| video.path).collect();
        paths.sort();
        paths
    }

    fn id(&self, path: &str) -> String {
        let listing = self.listing(&FolderQuery { below: true, ..self.query(None) });
        listing.videos.into_iter().find(|video| video.path == path).unwrap_or_else(|| panic!("{path} isn't listed")).movie.id
    }

    async fn read_again(&self) {
        self.core.read_folder_again(&self.source, &ignore).await.unwrap();
    }

    fn on_disk(&self, below: &str) -> bool {
        self.root.join(below).exists()
    }
}

fn facts(seconds: f64, width: i64, audio: bool, size: i64) -> FileFacts {
    FileFacts { duration_seconds: Some(seconds), width: Some(width), height: Some(width / 2), audio, size_bytes: Some(size) }
}

#[tokio::test]
async fn a_folder_lists_its_folders_and_videos_and_filters_them_by_what_they_hold() {
    let library = Library::new(&["meme.mp4", "ai clip.mp4", "song.mp4", "Imported/a.mp4", "Imported/Deeper/b.mp4", "Empty/notes.txt", ".hidden/c.mp4"]).await;
    let top = library.listing(&library.query(None));
    assert_eq!(top.trail.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), ["Videos"]);
    assert_eq!(top.folders.iter().map(|folder| (folder.name.as_str(), folder.videos)).collect::<Vec<_>>(), [("Empty", 0), ("Imported", 2)]);
    let mut here: Vec<&str> = top.videos.iter().map(|video| video.path.as_str()).collect();
    here.sort();
    assert_eq!(here, ["ai clip.mp4", "meme.mp4", "song.mp4"]);
    assert_eq!((top.total, top.unread), (3, 3));

    // One folder down, then everything below it.
    let imported = library.listing(&library.query(Some("Imported")));
    assert_eq!(imported.trail.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), ["Videos", "Imported"]);
    assert_eq!(imported.folders.iter().map(|folder| (folder.name.as_str(), folder.videos)).collect::<Vec<_>>(), [("Deeper", 1)]);
    assert_eq!(imported.videos.iter().map(|video| video.path.as_str()).collect::<Vec<_>>(), ["Imported/a.mp4"]);
    let below = library.listing(&FolderQuery { below: true, ..library.query(Some("Imported")) });
    assert_eq!(below.total, 2);

    // Once each file's been read, the filters place it.
    let keep = |path: &str, facts: FileFacts| library.core.keep_file_facts(&library.id(path), &library.source, Some(&facts)).unwrap();
    keep("meme.mp4", facts(3.0, 498, false, 90_000));
    keep("ai clip.mp4", facts(5.0, 720, false, 2_000_000));
    keep("song.mp4", facts(240.0, 1920, true, 50_000_000));
    let wanted = library.core.videos_without_facts(10).unwrap();
    assert_eq!(wanted.len(), 2, "Imported's two are still to read");
    let kinds = |kind: Option<ClipKind>, length: Option<ClipLength>| -> Vec<String> {
        library.listing(&FolderQuery { kind, length, ..library.query(None) }).videos.into_iter().map(|video| video.path).collect()
    };
    assert_eq!(kinds(Some(ClipKind::Gif), None), ["meme.mp4"]);
    assert_eq!(kinds(Some(ClipKind::Animation), None), ["ai clip.mp4"]);
    assert_eq!(kinds(Some(ClipKind::Video), None), ["song.mp4"]);
    assert_eq!(kinds(None, Some(ClipLength::Long)), ["song.mp4"]);
    assert_eq!(kinds(None, Some(ClipLength::Short)), ["ai clip.mp4", "meme.mp4"]);
    let listed = library.listing(&FolderQuery { sort: FolderSort::Length, ..library.query(None) });
    assert_eq!(listed.videos.iter().map(|video| (video.path.as_str(), video.kind)).collect::<Vec<_>>(), [
        ("song.mp4", Some(ClipKind::Video)),
        ("ai clip.mp4", Some(ClipKind::Animation)),
        ("meme.mp4", Some(ClipKind::Gif)),
    ]);
    assert_eq!(listed.videos[0].movie.duration_seconds, Some(240), "the length read from the file shows");
    assert_eq!(listed.unread, 0);

    // A folder outside the source's, or one hidden in it, isn't listed.
    assert!(library.core.folder_listing(&FolderQuery { folder: Some(library._folder.path().to_string_lossy().into_owned()), ..library.query(None) }).is_err());
    assert!(library.core.folder_listing(&library.query(Some(".hidden"))).is_err());
}

#[tokio::test]
async fn deleted_videos_and_folders_wait_in_recently_deleted_to_be_put_back_or_emptied() {
    let library = Library::new(&["keep.mp4", "meme.mp4", "meme.srt", "meme-poster.jpg", "meme-2.mp4", "meme-2.jpg", "Old/one.mp4", "Old/two.mp4"]).await;
    let meme = library.id("meme.mp4");
    let gone = library.core.delete_folder_items(&library.source, &[meme.clone(), library.id("Old/one.mp4")], &[library.path("Old")]).await.unwrap();
    assert_eq!(gone, 3, "meme, and Old's two with it");
    for path in ["meme.mp4", "meme.srt", "meme-poster.jpg", "Old"] {
        assert!(!library.on_disk(path), "{path} went");
    }
    assert!(library.on_disk("meme-2.jpg"), "another video's picture stays");
    assert_eq!(library.all(), ["keep.mp4", "meme-2.mp4"], "gone from the catalog at once");
    library.read_again().await;
    assert_eq!(library.all(), ["keep.mp4", "meme-2.mp4"], "and when read again: Recently Deleted is hidden");

    let batches = library.core.recently_deleted(&library.source).unwrap();
    assert_eq!(batches.len(), 1);
    let mut entries: Vec<(&str, bool)> = batches[0].entries.iter().map(|entry| (entry.path.as_str(), entry.folder)).collect();
    entries.sort();
    assert_eq!(entries, [("Old", true), ("meme-poster.jpg", false), ("meme.mp4", false), ("meme.srt", false)]);

    // Put back, beside a new file of the same name.
    make(&library.root, &["meme.mp4"]);
    library.core.restore_deleted(&library.source, &batches[0].id).await.unwrap();
    assert!(library.on_disk("meme (2).mp4") && library.on_disk("meme.srt") && library.on_disk("Old/two.mp4"));
    assert!(library.core.recently_deleted(&library.source).unwrap().is_empty());
    library.read_again().await;
    assert_eq!(library.all(), ["Old/one.mp4", "Old/two.mp4", "keep.mp4", "meme (2).mp4", "meme-2.mp4", "meme.mp4"]);

    // Deleted again, then for good.
    library.core.delete_folder_items(&library.source, &[library.id("keep.mp4")], &[]).await.unwrap();
    library.core.empty_recently_deleted(&library.source, None).await.unwrap();
    assert!(library.core.recently_deleted(&library.source).unwrap().is_empty());
    let bin = library.root.join(RECENTLY_DELETED);
    assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 0, "nothing left");
}

/// A folder its login can read but not change, as a network share can keep
/// one: nothing in it goes, it says why, and it leaves nothing behind.
#[cfg(unix)]
#[tokio::test]
async fn a_read_only_folder_says_so_and_keeps_everything() {
    use std::os::unix::fs::PermissionsExt;
    let library = Library::new(&["Locked/meme.mp4", "Locked/meme.mp4.json", "Open/clip.mp4"]).await;
    let locked = library.root.join("Locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let meme = library.id("Locked/meme.mp4");
    let outcome = library.core.delete_folder_items(&library.source, std::slice::from_ref(&meme), &[]).await;
    let moved = library.core.move_folder_items(&library.source, &[meme], &[], &library.path("Open")).await;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    let error = outcome.expect_err("refused").to_string();
    assert!(error.contains("“Locked” is read-only"), "{error}");
    assert!(moved.expect_err("refused").to_string().contains("read-only"));
    assert!(library.on_disk("Locked/meme.mp4") && library.on_disk("Locked/meme.mp4.json"));
    assert_eq!(library.all(), ["Locked/meme.mp4", "Open/clip.mp4"], "still listed");
    assert!(library.core.recently_deleted(&library.source).unwrap().is_empty());
    let bin = library.root.join(RECENTLY_DELETED);
    assert_eq!(std::fs::read_dir(&bin).map(|listing| listing.count()).unwrap_or(0), 0, "no empty deletion left behind");

    // The folder made writable, it goes, with what's named for it.
    library.core.delete_folder_items(&library.source, &[library.id("Locked/meme.mp4")], &[]).await.unwrap();
    assert!(!library.on_disk("Locked/meme.mp4") && !library.on_disk("Locked/meme.mp4.json"));
}

#[tokio::test]
async fn anything_in_recently_deleted_a_month_goes_for_good() {
    let library = Library::new(&["old.mp4", "new.mp4"]).await;
    library.core.delete_folder_items(&library.source, &[library.id("old.mp4")], &[]).await.unwrap();
    let old = library.core.recently_deleted(&library.source).unwrap().remove(0);
    let manifest = library.root.join(RECENTLY_DELETED).join(format!("{}.json", old.id));
    let aged = Manifest { deleted_at: now() - KEEP_DELETED_FOR - 60, entries: old.entries };
    std::fs::write(&manifest, serde_json::to_vec(&aged).unwrap()).unwrap();
    library.core.delete_folder_items(&library.source, &[library.id("new.mp4")], &[]).await.unwrap();
    let left = library.core.recently_deleted(&library.source).unwrap();
    assert_eq!(left.iter().map(|batch| batch.entries[0].path.as_str()).collect::<Vec<_>>(), ["new.mp4"]);
    assert!(!manifest.exists() && !library.root.join(RECENTLY_DELETED).join(&old.id).exists());
}

#[tokio::test]
async fn moved_and_renamed_videos_keep_their_progress_and_what_was_found_for_them() {
    let library = Library::new(&["clip.mp4", "clip.srt", "other.mp4", "Memes/clip.mp4", "Box/inside.mp4"]).await;
    let clip = library.id("clip.mp4");
    let progress = |id: &str| PlaybackProgress {
        item_id: id.into(),
        kind: MediaKind::Movie,
        source_id: library.source.clone(),
        series_id: None,
        position_seconds: 12.0,
        duration_seconds: 30.0,
        finished: false,
        updated_at: 0,
    };
    library.core.save_progress(progress(&clip)).unwrap();
    library.core.keep_file_facts(&clip, &library.source, Some(&facts(30.0, 400, false, 10_000))).unwrap();
    library.core.keep_video_picture(&clip, &library.source, "xorvio-still://localhost/0123456789abcdef0123456789abcdef.jpg").unwrap();

    // Into a folder that already has a clip.mp4: it's renamed beside it, and its subtitles go too.
    library.core.move_folder_items(&library.source, std::slice::from_ref(&clip), &[], &library.path("Memes")).await.unwrap();
    assert!(library.on_disk("Memes/clip (2).mp4") && library.on_disk("Memes/clip (2).srt") && !library.on_disk("clip.mp4"));
    let moved = library.id("Memes/clip (2).mp4");
    assert_ne!(moved, clip);
    let kept = |id: &str| {
        let movie = library.core.movie(id).unwrap().expect("listed");
        (library.core.progress(id).unwrap().map(|progress| progress.position_seconds), movie.poster_url.is_some(), library.core.catalog.videos_without_facts(std::slice::from_ref(&library.source), 100).unwrap().iter().any(|(other, _, _)| other == id))
    };
    assert_eq!(kept(&moved), (Some(12.0), true, false), "progress, frame and facts moved with it");
    library.read_again().await;
    assert_eq!(library.id("Memes/clip (2).mp4"), moved, "the id it was given is the one the scan gives");
    assert_eq!(kept(&moved), (Some(12.0), true, false));

    // Renamed: the id follows again.
    library.core.rename_folder_video(&library.source, &moved, "Dancing cat").await.unwrap();
    assert!(library.on_disk("Memes/Dancing cat.mp4") && library.on_disk("Memes/Dancing cat.srt"));
    let renamed = library.id("Memes/Dancing cat.mp4");
    assert_eq!(kept(&renamed).0, Some(12.0));
    assert!(library.core.rename_folder_video(&library.source, &renamed, "clip").await.is_err(), "clip.mp4 is taken there");
    assert!(library.core.rename_folder_video(&library.source, &renamed, "../escape").await.is_err());

    // A folder moved, with everything in it.
    let inside = library.id("Box/inside.mp4");
    library.core.save_progress(progress(&inside)).unwrap();
    library.core.move_folder_items(&library.source, &[], &[library.path("Box")], &library.path("Memes")).await.unwrap();
    let moved_inside = library.id("Memes/Box/inside.mp4");
    assert_eq!(library.core.progress(&moved_inside).unwrap().map(|progress| progress.position_seconds), Some(12.0));
    assert!(library.core.move_folder_items(&library.source, &[], &[library.path("Memes")], &library.path("Memes/Box")).await.is_err(), "not into itself");
    library.read_again().await;
    assert_eq!(library.all(), ["Memes/Box/inside.mp4", "Memes/Dancing cat.mp4", "Memes/clip.mp4", "other.mp4"]);
}

#[tokio::test]
async fn folders_are_made_renamed_and_kept_inside_the_source() {
    let library = Library::new(&["Clips/a.mp4"]).await;
    let made = library.core.create_media_folder(&library.source, &library.path(""), "GIFs").await.unwrap();
    assert!(Path::new(&made).is_dir());
    assert!(library.core.create_media_folder(&library.source, &library.path(""), "GIFs").await.is_err(), "already there");
    for bad in ["", ".secret", "a/b"] {
        assert!(library.core.create_media_folder(&library.source, &library.path(""), bad).await.is_err(), "{bad:?}");
    }
    assert!(library.core.create_media_folder(&library.source, &library._folder.path().to_string_lossy(), "Out").await.is_err(), "outside the source");
    assert!(library.core.rename_media_folder(&library.source, &library.path(""), "Renamed").await.is_err(), "the source's own folder stays");
    assert!(library.core.delete_folder_items(&library.source, &[], &[library.path("")]).await.is_err());

    let a = library.id("Clips/a.mp4");
    library.core.rename_media_folder(&library.source, &library.path("Clips"), "Sorted").await.unwrap();
    assert!(library.on_disk("Sorted/a.mp4"));
    assert_ne!(library.id("Sorted/a.mp4"), a);
    library.read_again().await;
    let listing = library.listing(&library.query(None));
    assert_eq!(listing.folders.iter().map(|folder| (folder.name.as_str(), folder.videos)).collect::<Vec<_>>(), [("GIFs", 0), ("Sorted", 1)]);
}

#[test]
fn names_and_kept_paths_are_checked() {
    assert_eq!(valid_name("  Memes  ").unwrap(), "Memes");
    for bad in ["", "   ", ".hidden", "a/b", "a\\b", "a:b"] {
        assert!(valid_name(bad).is_err(), "{bad:?}");
    }
    assert!(checked_relative("Memes/clip.mp4").is_ok());
    for bad in ["", "../outside.mp4", "Memes/../../x", "/etc/passwd"] {
        assert!(checked_relative(bad).is_err(), "{bad:?}");
    }
    assert!(valid_batch_id(&batch_id()));
    assert!(!valid_batch_id("../x"));
}

#[test]
fn paths_must_be_inside_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Videos");
    std::fs::create_dir_all(root.join("Memes")).unwrap();
    std::fs::create_dir_all(root.join(".hidden")).unwrap();
    let roots = vec![root.to_string_lossy().into_owned()];
    assert!(inside(&roots, &root.join("Memes"), false).is_ok());
    assert!(inside(&roots, &root, true).is_ok());
    assert!(inside(&roots, &root, false).is_err(), "the source's own folder can't be moved or deleted");
    assert!(inside(&roots, &root.join(".hidden"), false).is_err());
    assert!(inside(&roots, &root.join("Memes/../.."), true).is_err());
    assert!(inside(&roots, dir.path(), true).is_err());
    assert!(inside(&roots, Path::new("Memes"), true).is_err());
    #[cfg(unix)]
    {
        std::fs::write(dir.path().join("outside.mp4"), b"").unwrap();
        std::os::unix::fs::symlink(dir.path(), root.join("Away")).unwrap();
        let mut disk = Disk::default();
        let away = inside(&roots, &root.join("Away"), true).unwrap();
        assert!(disk.check(&away, true).is_err(), "a link that leads out, to list or move into");
        assert!(disk.check(&away, false).is_ok(), "the link itself can be moved: what it leads to isn't touched");
        assert!(disk.check(&inside(&roots, &root.join("Away/outside.mp4"), false).unwrap(), false).is_err(), "a file reached through it");
        assert!(disk.check(&inside(&roots, &root.join("Memes"), false).unwrap(), true).is_ok());
        assert!(disk.check_new(&roots[0], &root.join("Memes/New/clip.mp4")).is_ok());
        assert!(disk.check_new(&roots[0], &root.join("Away/New/clip.mp4")).is_err());
    }
}

#[test]
fn whats_beside_a_video_goes_with_it_but_never_another_video() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["Clip.mp4", "Clip.srt", "Clip-poster.jpg", "Clip.nfo", "Clip.mp4.json", "Clip.mp4.xmp", "Clip.mp4.mkv", "Clip-2.mp4", "Clip-2.jpg", "Clip 2.jpg", "Other.png"] {
        std::fs::write(dir.path().join(name), b"").unwrap();
    }
    let mut disk = Disk::default();
    let mut found: Vec<String> = disk.beside(&dir.path().join("Clip.mp4")).iter().map(|path| path.file_name().unwrap().to_string_lossy().into_owned()).collect();
    found.sort();
    assert_eq!(found, ["Clip-poster.jpg", "Clip.mp4.json", "Clip.mp4.xmp", "Clip.nfo", "Clip.srt"], "never another video, even one named for it");
    assert_eq!(disk.unused(&dir.path().join("Clip.mp4")).file_name().unwrap(), "Clip (2).mp4");
    assert_eq!(disk.unused(&dir.path().join("clip.MP4")).file_name().unwrap(), "clip (3).MP4", "names are compared as most disks do, in any case: Clip.mp4 and the Clip (2).mp4 just planned are both taken");
    assert_eq!(disk.unused(&dir.path().join("New.mp4")).file_name().unwrap(), "New.mp4");
    assert_eq!(disk.unused(&dir.path().join("New.mp4")).file_name().unwrap(), "New (2).mp4", "taken by another file in the same change");

    // Renaming never replaces what's there.
    let (from, to) = (dir.path().join("Other.png"), dir.path().join("Clip.srt"));
    assert!(renamed(&from, &to).is_err());
    assert!(from.exists() && to.exists());
    renamed(&from, &dir.path().join("Other renamed.png")).unwrap();
}
