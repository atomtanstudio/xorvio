//! Reading a media folder's files. The whole tree is walked, several
//! directories at once (which is what makes a network share quick), and
//! compared with the last walk by size and modification time, so only .nfo
//! files that changed are read again. Videos are never opened here: they're
//! probed when they play.
//!
//! The last walk of each folder is kept in a small file beside the catalog.
//! A folder that can't be reached (a drive unplugged, a share not mounted)
//! is listed from it, so its films and series stay rather than vanish.

use std::collections::{BTreeMap, HashMap};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::naming::{self, IMAGE_EXTENSIONS, VIDEO_EXTENSIONS};
use super::nfo::{self, Nfo};
use super::organize;
use crate::error::SourceError;
use crate::snapshot::{CatalogSnapshot, grouped};
use crate::store::lock;

/// Directories read at once.
const WORKERS: usize = 8;
/// More than any real library has; a share pointed at a whole disk stops here.
pub(super) const MOST_FILES: usize = 200_000;
/// How long a folder has to answer before it counts as not connected: a
/// share whose server has gone away can otherwise hang for minutes.
const ANSWER_WITHIN: Duration = Duration::from_secs(10);
/// A big directory's files are looked at in batches this size, shared out.
const BATCH: usize = 64;
/// Bump when [`Listing`] changes meaning, so older scans are read afresh.
const CACHE_VERSION: u32 = 1;

/// Folders nobody keeps videos in: a NAS's housekeeping and the system's own.
const SKIPPED_FOLDERS: &[&str] = &["@eadir", "@recycle", "#recycle", "#snapshot", "$recycle.bin", "system volume information", "lost+found"];
/// macOS packages, which look like folders but are an app's own.
const PACKAGES: &[&str] = &["app", "bundle", "framework", "photoslibrary", "photolibrary", "imovielibrary", "fcpbundle", "tvlibrary", "musiclibrary"];

/// A folder's videos, .nfo files and images, as last read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Listing {
    /// In path order.
    pub files: Vec<FileEntry>,
    /// The folder had more files than are read ([`MOST_FILES`]).
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FileEntry {
    /// Below the folder, "/" between parts, as the disk spells it.
    pub path: String,
    pub size: u64,
    /// Milliseconds since 1970.
    pub modified: i64,
    /// When it came to be there, in seconds: its creation time, or its
    /// modification time where the disk doesn't keep one.
    pub added: i64,
    /// An .nfo file's contents (empty when it says nothing usable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nfo: Option<Nfo>,
}

/// Why a folder couldn't be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Unavailable {
    Missing,
    NotResponding,
    NotAllowed,
}

impl Unavailable {
    fn sentence(self, folder: &str) -> String {
        let name = naming::folder_name(folder);
        match self {
            Self::Missing => format!("“{name}” isn't available right now. Connect the drive or network share it's on, then refresh."),
            Self::NotResponding => format!("“{name}” isn't responding. Check the drive or network share it's on, then refresh."),
            Self::NotAllowed if cfg!(target_os = "macos") => format!(
                "Xorvio isn't allowed to read “{name}”. Allow it in System Settings › Privacy & Security › Files and Folders, then refresh."
            ),
            Self::NotAllowed => format!("Xorvio isn't allowed to read “{name}”. Check the folder's permissions, then refresh."),
        }
    }
}

/// Every folder's last walk, kept beside the catalog.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Cache {
    version: u32,
    folders: BTreeMap<String, Listing>,
}

impl Cache {
    fn load(path: &Path) -> Cache {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
            .filter(|cache| cache.version == CACHE_VERSION)
            .unwrap_or_default()
    }

    /// Replaces the file in one step, so a crash never leaves half of one.
    fn save(&self, path: &Path) -> anyhow::Result<()> {
        let directory = path.parent().ok_or_else(|| anyhow::anyhow!("the scan has nowhere to be kept"))?;
        std::fs::create_dir_all(directory)?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        serde_json::to_writer(std::io::BufWriter::new(&mut temporary), self)?;
        temporary.persist(path)?;
        Ok(())
    }
}

/// Reads a media folder source's folders and makes its catalog. Folders that
/// can't be reached are listed as they were last time, with a warning; when
/// none can, this fails and the catalog stays as it is. `cache` is where
/// each folder's walk is kept between scans.
pub(crate) fn scan(source_id: &str, folders: &[String], cache: &Path) -> Result<CatalogSnapshot, SourceError> {
    if folders.is_empty() {
        return Err(SourceError::MissingConfiguration("a folder".into()));
    }
    let mut kept = Cache::load(cache);
    let mut read: Vec<(String, Listing)> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for folder in folders {
        let previous = kept.folders.get(folder);
        match read_folder(Path::new(folder), previous) {
            Ok(listing) => read.push((folder.clone(), listing)),
            Err(why) => {
                let mut sentence = why.sentence(folder);
                if let Some(previous) = previous {
                    sentence.push_str(" What was in it before is still listed.");
                    read.push((folder.clone(), previous.clone()));
                }
                problems.push(sentence);
            }
        }
    }
    if problems.len() == folders.len() {
        return Err(SourceError::Unavailable(problems.join(" ")));
    }
    kept = Cache { version: CACHE_VERSION, folders: read.iter().cloned().collect() };
    if let Err(error) = kept.save(cache) {
        // The catalog is still right; the next scan just reads every .nfo again.
        tracing::warn!("a media folder's scan couldn't be kept: {error}");
    }
    let mut snapshot = organize::snapshot(source_id, &read);
    snapshot.warnings.extend(problems);
    for (folder, listing) in &read {
        if listing.truncated {
            snapshot.warnings.push(format!(
                "“{}” has more than {} files, so only the first {} were read.",
                naming::folder_name(folder),
                grouped(MOST_FILES as i64),
                grouped(MOST_FILES as i64)
            ));
        }
    }
    Ok(snapshot)
}

/// Removes a source's kept scans.
pub(crate) fn forget(cache: &Path) {
    if let Err(error) = std::fs::remove_file(cache)
        && error.kind() != ErrorKind::NotFound
    {
        tracing::warn!("a media folder's scan couldn't be removed: {error}");
    }
}

/// One folder's files, with the .nfo files that changed read again.
fn read_folder(root: &Path, previous: Option<&Listing>) -> Result<Listing, Unavailable> {
    check_reachable(root)?;
    let (mut listing, missed) = walk(root);
    keep_missed(&mut listing, missed, previous);
    read_nfos(root, &mut listing, previous);
    Ok(listing)
}

/// Files the folder lists but that couldn't be looked at stay, as they were
/// last time or with nothing known. A Mac's network share can list a name it
/// then can't find (one written to the server in decomposed Unicode, which
/// the Mac looks up composed), so dropping them would take them out of the
/// library on one scan and put them back on another.
fn keep_missed(listing: &mut Listing, missed: Vec<String>, previous: Option<&Listing>) {
    if missed.is_empty() {
        return;
    }
    let before: HashMap<&str, &FileEntry> = previous.map(|listing| listing.files.iter().map(|file| (file.path.as_str(), file)).collect()).unwrap_or_default();
    for path in missed {
        let entry = match before.get(path.as_str()) {
            Some(old) => (*old).clone(),
            None => FileEntry { path, size: 0, modified: 0, added: 0, nfo: None },
        };
        listing.files.push(entry);
    }
    listing.files.sort_by(|a, b| a.path.cmp(&b.path));
}

fn check_reachable(root: &Path) -> Result<(), Unavailable> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let path = root.to_path_buf();
    // On a thread of its own, so a share that hangs only holds up that thread.
    std::thread::spawn(move || {
        let _ = sender.send(std::fs::read_dir(&path).map(|mut entries| entries.next().is_some()));
    });
    match receiver.recv_timeout(ANSWER_WITHIN) {
        Err(_) => Err(Unavailable::NotResponding),
        Ok(Err(error)) if error.kind() == ErrorKind::PermissionDenied => Err(Unavailable::NotAllowed),
        Ok(Err(_)) => Err(Unavailable::Missing),
        // A share that dropped can leave its mount point behind, empty.
        // Listing nothing from it would empty the library, so it counts as not connected.
        Ok(Ok(false)) if is_mount_point(root) => Err(Unavailable::Missing),
        Ok(Ok(_)) => Ok(()),
    }
}

/// A whole drive or share: "/Volumes/NAS", "/media/me/Drive", "/mnt/nas".
fn is_mount_point(path: &Path) -> bool {
    let parts: Vec<_> = path.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
    matches!(
        parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice(),
        ["/", "Volumes" | "mnt" | "media", _] | ["/", "media", _, _] | ["/", "run", "media", _, _]
    )
}

fn wanted(name: &str) -> bool {
    naming::has_extension(name, VIDEO_EXTENSIONS) || naming::has_extension(name, IMAGE_EXTENSIONS) || naming::has_extension(name, &["nfo"])
}

/// Folders not walked into: a NAS's housekeeping, packages, and extras
/// (which [`naming::classify`] would leave out anyway).
pub(crate) fn skipped_folder(name: &str, depth: usize) -> bool {
    let lower = name.to_lowercase();
    SKIPPED_FOLDERS.contains(&lower.as_str())
        || naming::extension(&lower).is_some_and(|extension| PACKAGES.contains(&extension.as_str()))
        || naming::is_extras_folder(name, depth)
}

enum Job {
    Folder { path: PathBuf, relative: String, depth: usize },
    Files(Vec<Listed>),
}

/// A file a directory listed: where it is, its path below the folder, and
/// whether it's a link.
#[derive(Clone)]
struct Listed {
    path: PathBuf,
    relative: String,
    link: bool,
}

#[derive(Default)]
struct Queue {
    jobs: Vec<Job>,
    /// Workers in the middle of a job, which may add more.
    busy: usize,
}

struct Walk {
    queue: Mutex<Queue>,
    changed: Condvar,
    seen: AtomicUsize,
    /// Files listed that couldn't be looked at.
    missed: Mutex<Vec<String>>,
}

/// Marks a job finished however it ends, so the other workers never wait
/// on one that's gone.
struct Done<'a>(&'a Walk);

impl Drop for Done<'_> {
    fn drop(&mut self) {
        let mut queue = lock(&self.0.queue);
        queue.busy -= 1;
        if queue.busy == 0 && queue.jobs.is_empty() {
            self.0.changed.notify_all();
        }
    }
}

/// The folder's files, and the paths of those listed that couldn't be looked at.
fn walk(root: &Path) -> (Listing, Vec<String>) {
    let walk = Walk {
        queue: Mutex::new(Queue { jobs: vec![Job::Folder { path: root.to_path_buf(), relative: String::new(), depth: 0 }], busy: 0 }),
        changed: Condvar::new(),
        seen: AtomicUsize::new(0),
        missed: Mutex::new(Vec::new()),
    };
    let mut files: Vec<FileEntry> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..WORKERS).map(|_| scope.spawn(|| walk.work())).collect();
        workers.into_iter().flat_map(|worker| worker.join().unwrap_or_default()).collect()
    });
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let listing = Listing { files, truncated: walk.seen.load(Ordering::Relaxed) > MOST_FILES };
    (listing, walk.missed.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner))
}

impl Walk {
    fn work(&self) -> Vec<FileEntry> {
        let mut found = Vec::new();
        while let Some(job) = self.next() {
            let _done = Done(self);
            match job {
                Job::Folder { path, relative, depth } => self.read(&path, &relative, depth, &mut found),
                Job::Files(files) => self.look(&files, &mut found),
            }
        }
        found
    }

    /// The next job, waiting while others may still add some; `None` once
    /// there's nothing left anywhere.
    fn next(&self) -> Option<Job> {
        let mut queue = lock(&self.queue);
        loop {
            if let Some(job) = queue.jobs.pop() {
                queue.busy += 1;
                return Some(job);
            }
            if queue.busy == 0 {
                return None;
            }
            queue = self.changed.wait(queue).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn add(&self, jobs: Vec<Job>) {
        if !jobs.is_empty() {
            lock(&self.queue).jobs.extend(jobs);
            self.changed.notify_all();
        }
    }

    /// One directory: its folders become jobs, and its files are looked at
    /// here, or shared out when there are many.
    fn read(&self, path: &Path, relative: &str, depth: usize, found: &mut Vec<FileEntry>) {
        // A folder inside that can't be read is passed over; the rest still lists.
        let Ok(entries) = std::fs::read_dir(path) else { return };
        let mut jobs = Vec::new();
        let mut files = Vec::new();
        for entry in entries.flatten() {
            if self.seen.fetch_add(1, Ordering::Relaxed) >= MOST_FILES {
                break;
            }
            let name = entry.file_name();
            // Names that aren't text can't be shown or matched.
            let Some(name) = name.to_str() else { continue };
            if name.starts_with('.') {
                continue;
            }
            let child = if relative.is_empty() { name.to_string() } else { format!("{relative}/{name}") };
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if !skipped_folder(name, depth) {
                    jobs.push(Job::Folder { path: entry.path(), relative: child, depth: depth + 1 });
                }
            } else if (kind.is_file() || kind.is_symlink()) && wanted(name) {
                // Linked files are followed; linked folders aren't, so a link can't loop.
                files.push(Listed { path: entry.path(), relative: child, link: kind.is_symlink() });
            }
        }
        if files.len() <= BATCH {
            self.look(&files, found);
        } else {
            jobs.extend(files.chunks(BATCH).map(|batch| Job::Files(batch.to_vec())));
        }
        self.add(jobs);
    }

    /// These files' sizes and times, noting the ones that couldn't be looked at.
    fn look(&self, files: &[Listed], found: &mut Vec<FileEntry>) {
        for file in files {
            match file_entry(&file.path, &file.relative) {
                Some(entry) => found.push(entry),
                // A link that leads nowhere isn't a file.
                None if file.link => {}
                None => lock(&self.missed).push(file.relative.clone()),
            }
        }
    }
}

fn file_entry(path: &Path, relative: &str) -> Option<FileEntry> {
    let metadata = std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file)?;
    let since_1970 = |time: SystemTime| time.duration_since(UNIX_EPOCH).ok();
    let modified = metadata.modified().ok().and_then(since_1970).unwrap_or_default();
    let added = metadata.created().ok().and_then(since_1970).unwrap_or(modified);
    Some(FileEntry {
        path: relative.to_string(),
        size: metadata.len(),
        modified: i64::try_from(modified.as_millis()).unwrap_or(i64::MAX),
        added: i64::try_from(added.as_secs()).unwrap_or(i64::MAX),
        nfo: None,
    })
}

/// Fills in each .nfo file's contents: kept from last time when its size and
/// modification time are the same, otherwise read again, several at once.
fn read_nfos(root: &Path, listing: &mut Listing, previous: Option<&Listing>) {
    let before: HashMap<&str, &FileEntry> =
        previous.map(|listing| listing.files.iter().filter(|file| file.nfo.is_some()).map(|file| (file.path.as_str(), file)).collect()).unwrap_or_default();
    let mut to_read: Vec<(usize, PathBuf)> = Vec::new();
    for (index, file) in listing.files.iter_mut().enumerate() {
        if !naming::has_extension(&file.path, &["nfo"]) {
            continue;
        }
        match before.get(file.path.as_str()) {
            Some(old) if old.size == file.size && old.modified == file.modified => file.nfo = old.nfo.clone(),
            _ => to_read.push((index, root.join(&file.path))),
        }
    }
    if to_read.is_empty() {
        return;
    }
    let share = to_read.len().div_ceil(WORKERS);
    let read: Vec<(usize, Nfo)> = std::thread::scope(|scope| {
        let workers: Vec<_> = to_read
            .chunks(share)
            .map(|batch| scope.spawn(move || batch.iter().map(|(index, path)| (*index, read_nfo(path))).collect::<Vec<_>>()))
            .collect();
        workers.into_iter().flat_map(|worker| worker.join().unwrap_or_default()).collect()
    });
    for (index, nfo) in read {
        listing.files[index].nfo = Some(nfo);
    }
}

/// An .nfo file's contents, or empty ones when it can't be read or says nothing.
fn read_nfo(path: &Path) -> Nfo {
    let small = std::fs::metadata(path).is_ok_and(|metadata| metadata.len() <= nfo::MAX_BYTES);
    small.then(|| std::fs::read(path).ok()).flatten().and_then(|bytes| Nfo::parse(&bytes)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Windows has none to leave behind: a drive or share that's away can't be read.
    #[cfg(unix)]
    #[test]
    fn mount_points() {
        assert!(is_mount_point(Path::new("/Volumes/NAS")));
        assert!(is_mount_point(Path::new("/media/me/Drive")));
        assert!(!is_mount_point(Path::new("/Volumes/NAS/Films")));
        assert!(!is_mount_point(Path::new("/Users/me/Movies")));
    }

    #[cfg(unix)]
    #[test]
    fn files_listed_but_not_looked_at_stay() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("Locked")).unwrap();
        std::fs::write(root.join("Locked/Jónsi - Tornado.mp4"), b"video").unwrap();
        std::fs::write(root.join("Heat.mp4"), b"video").unwrap();
        std::os::unix::fs::symlink(root.join("gone.mp4"), root.join("Link.mp4")).unwrap();
        // A folder that can be listed but not looked into, as a share's
        // misspelled names are: its names can be read, not their sizes.
        std::fs::set_permissions(root.join("Locked"), std::fs::Permissions::from_mode(0o444)).unwrap();
        let unreachable = std::fs::metadata(root.join("Locked/Jónsi - Tornado.mp4")).is_err();
        let (listing, missed) = walk(root);
        std::fs::set_permissions(root.join("Locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
        if !unreachable {
            return; // Run with privileges that see through permissions.
        }
        assert_eq!(listing.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(), ["Heat.mp4"]);
        // The link that leads nowhere isn't kept.
        assert_eq!(missed, ["Locked/Jónsi - Tornado.mp4"]);
    }

    #[test]
    fn missed_files_keep_what_was_known() {
        let file = |path: &str, size: u64| FileEntry { path: path.into(), size, modified: 7, added: 9, nfo: None };
        let previous = Listing { files: vec![file("B.mp4", 500), file("Gone.mp4", 1)], truncated: false };
        let mut listing = Listing { files: vec![file("A.mp4", 10), file("D.mp4", 40)], truncated: false };
        keep_missed(&mut listing, vec!["C.mp4".into(), "B.mp4".into()], Some(&previous));
        assert_eq!(
            listing.files,
            [file("A.mp4", 10), file("B.mp4", 500), FileEntry { path: "C.mp4".into(), size: 0, modified: 0, added: 0, nfo: None }, file("D.mp4", 40)]
        );
    }

    #[test]
    fn housekeeping_and_packages_are_passed_over() {
        assert!(skipped_folder("@eaDir", 0));
        assert!(skipped_folder("#recycle", 3));
        assert!(skipped_folder("Photos Library.photoslibrary", 0));
        assert!(skipped_folder("Extras", 1));
        assert!(!skipped_folder("Films", 0));
        assert!(!skipped_folder("Mr. Robot", 0));
    }
}
