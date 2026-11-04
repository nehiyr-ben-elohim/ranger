//! Scans the home folder and moves media/documents into the standard macOS folders.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::io;
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Pictures,
    Music,
    Movies,
    Documents,
    Archives,
    Installers,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Pictures,
        Category::Music,
        Category::Movies,
        Category::Documents,
        Category::Archives,
        Category::Installers,
    ];

}

/// The user's real target folders: macOS defaults, or on Linux the XDG user dirs
/// from ~/.config/user-dirs.dirs (often localized: ~/Images, ~/Vidéos...).
#[derive(Clone, Debug)]
pub struct Layout {
    pub home: PathBuf,
    pictures: PathBuf,
    music: PathBuf,
    videos: PathBuf,
    documents: PathBuf,
    downloads: PathBuf,
}

impl Layout {
    pub fn for_home(home: &Path) -> Self {
        let videos = if cfg!(target_os = "macos") { "Movies" } else { "Videos" };
        let mut layout = Layout {
            home: home.to_path_buf(),
            pictures: home.join("Pictures"),
            music: home.join("Music"),
            videos: home.join(videos),
            documents: home.join("Documents"),
            downloads: home.join("Downloads"),
        };
        if cfg!(target_os = "linux") {
            if let Ok(text) = fs::read_to_string(home.join(".config/user-dirs.dirs")) {
                layout.apply_user_dirs(&text);
            }
        }
        layout
    }

    /// Reads lines like `XDG_PICTURES_DIR="$HOME/Images"`. A dir set to $HOME itself means "disabled".
    pub fn apply_user_dirs(&mut self, text: &str) {
        for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim().trim_matches('"');
            let path = if let Some(rest) = value.strip_prefix("$HOME") {
                self.home.join(rest.trim_start_matches('/'))
            } else if value.starts_with('/') {
                PathBuf::from(value)
            } else {
                continue;
            };
            if path == self.home || !path.starts_with(&self.home) {
                continue;
            }
            match key.trim() {
                "XDG_PICTURES_DIR" => self.pictures = path,
                "XDG_MUSIC_DIR" => self.music = path,
                "XDG_VIDEOS_DIR" => self.videos = path,
                "XDG_DOCUMENTS_DIR" => self.documents = path,
                "XDG_DOWNLOAD_DIR" => self.downloads = path,
                _ => {}
            }
        }
    }

    /// Absolute target folder. Neither OS has a standard place for archives/installers.
    pub fn root(&self, c: Category) -> PathBuf {
        match c {
            Category::Pictures => self.pictures.clone(),
            Category::Music => self.music.clone(),
            Category::Movies => self.videos.clone(),
            Category::Documents => self.documents.clone(),
            Category::Archives => self.documents.join("Archives"),
            Category::Installers => self.downloads.join("Installeurs"),
        }
    }

    /// Target folder relative to home, for display and the AI prompt.
    pub fn folder(&self, c: Category) -> String {
        let root = self.root(c);
        root.strip_prefix(&self.home).unwrap_or(&root).to_string_lossy().into_owned()
    }

    /// Folders that are already sorted: skipped unless re-sorting is asked for.
    fn is_sorted_root(&self, dir: &Path) -> bool {
        [&self.pictures, &self.music, &self.videos, &self.documents].iter().any(|r| *r == dir)
    }
}

/// Single source of truth for extensions, also sent to the UI.
pub const CATALOG: &[(Category, &[&str])] = &[
    (
        Category::Pictures,
        &[
            "jpg", "jpeg", "png", "gif", "heic", "heif", "webp", "bmp", "tif", "tiff", "svg",
            "raw", "cr2", "cr3", "nef", "arw", "dng", "psd", "avif", "jxl", "ico", "ai", "eps",
            "sketch", "fig", "xd", "afdesign", "afphoto", "xcf",
        ],
    ),
    (
        Category::Music,
        &[
            "mp3", "m4a", "aac", "wav", "flac", "aiff", "aif", "ogg", "opus", "wma", "alac",
            "mid", "midi", "amr", "caf", "m4r", "m4b",
        ],
    ),
    (
        Category::Movies,
        // No "ts": clashes with TypeScript
        &[
            "mp4", "mov", "m4v", "avi", "mkv", "wmv", "flv", "webm", "mpg", "mpeg", "3gp",
            "mts", "m2ts", "vob", "ogv", "srt", "vtt",
        ],
    ),
    (
        Category::Documents,
        &[
            "pdf", "doc", "docx", "odt", "rtf", "txt", "md", "pages", "xls", "xlsx", "ods",
            "csv", "numbers", "ppt", "pptx", "odp", "key", "epub", "xlsm", "xlsb", "dotx",
            "xltx", "potx", "odg", "mobi", "azw3", "djvu", "vcf", "ics",
        ],
    ),
    (
        Category::Archives,
        &["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "zst", "cab"],
    ),
    (Category::Installers, &["dmg", "pkg", "mpkg", "iso", "exe", "msi", "apk", "ipa"]),
];

fn extension_of(path: &Path) -> Option<String> {
    Some(path.extension()?.to_str()?.to_ascii_lowercase())
}

pub fn category_for(path: &Path) -> Option<Category> {
    let ext = extension_of(path)?;
    CATALOG
        .iter()
        .find(|(_, exts)| exts.contains(&ext.as_str()))
        .map(|(c, _)| *c)
}

/// Top-level folders of $HOME never scanned: system, already sorted, cloud sync, dev tools.
fn is_skipped_top_level(name: &str) -> bool {
    const NAMES: &[&str] = &[
        // system
        "library", "public", "snap", "flatpak", "appimages",
        // cloud sync
        "dropbox", "google drive", "creative cloud files", "icloud drive",
        // dev tools, SDKs, VMs
        "orbstack", "fvm", "go", "flutter", "sdk", "android", "developer", "development",
        "miniconda3", "anaconda3", "miniforge3", "mambaforge", "homebrew", "opt", "bin",
        "parallels", "virtualbox vms", "vagrant", "postman", "docker", "perl5", "venv",
        "node_modules", "workspace", "workspaces", "git", "github", "gitlab", "repos", "code",
    ];
    let lower = name.to_lowercase();
    NAMES.contains(&lower.as_str())
        || lower.starts_with("onedrive")
        || lower.starts_with("applications")
        // IdeaProjects, PycharmProjects, AndroidStudioProjects...
        || lower.ends_with("projects")
}

/// App libraries living inside Pictures/Music/Movies/Documents: moving their files breaks the app.
fn is_app_library(home: &Path, dir: &Path) -> bool {
    const PATHS: &[&str] = &[
        "Music/Music", "Music/iTunes", "Music/GarageBand", "Music/Logic", "Music/Audio Music Apps",
        "Movies/TV", "Movies/Motion Templates", "Movies/Motion Templates.localized",
        "Movies/Final Cut Backups.localized",
    ];
    let name = dir.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    // "Photo Booth Library", "iPhoto Library", "Aperture Library"...
    name.ends_with(" library")
        || dir.strip_prefix(home).map(|rel| PATHS.iter().any(|p| rel == Path::new(p))).unwrap_or(false)
}

/// macOS packages: folders that Finder shows as a single file.
fn is_package(dir: &Path) -> bool {
    const EXTS: &[&str] = &[
        "app", "photoslibrary", "musiclibrary", "tvlibrary", "imovielibrary", "fcpbundle",
        "logicx", "band", "bundle", "framework", "xcodeproj", "xcworkspace", "xcassets",
        "pvm", "vmwarevm", "utm", "vbox", "dmg", "sparsebundle",
        "rtfd", "pages", "numbers", "key", "lrdata", "lrlibrary", "aplibrary", "photolibrary",
    ];
    dir.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Code projects: moving their files would break them.
fn is_project_dir(dir: &Path) -> bool {
    const MARKERS: &[&str] = &[
        ".git", "package.json", "Cargo.toml", "composer.json", "pyproject.toml",
        "requirements.txt", "go.mod", "pom.xml", "build.gradle", "Gemfile", "pyvenv.cfg",
        "Podfile", "pubspec.yaml", "node_modules", ".fvm", ".venv",
        "venv", ".dart_tool", "Makefile", "Dockerfile", "docker-compose.yml", "compose.yaml",
        "CMakeLists.txt", "build.gradle.kts", "settings.gradle", "mix.exs", "deno.json",
        "tsconfig.json", "setup.py", "Pipfile",
    ];
    MARKERS.iter().any(|m| dir.join(m).exists())
}

/// ~/Downloads/Trip/a.jpg -> ~/Pictures/Trip/a.jpg ; ~/a.jpg -> ~/Pictures/a.jpg
pub fn destination(layout: &Layout, src: &Path, cat: Category) -> Option<PathBuf> {
    let home = &layout.home;
    let rel = src.strip_prefix(home).ok()?;
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    // Already under its target folder (e.g. ~/Downloads/Installeurs/x.dmg): stays put
    let root = layout.root(cat);
    if src.starts_with(&root) {
        return Some(src.to_path_buf());
    }
    let mut comps = rel.components();
    let kept: PathBuf = if rel.components().count() > 1 {
        comps.next();
        comps.collect()
    } else {
        rel.to_path_buf()
    };
    Some(root.join(kept))
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub src: PathBuf,
    pub dest: PathBuf,
    pub category: Category,
    pub size: u64,
    /// Absolute target folder, for AI subfolder previews in the UI.
    pub target_root: PathBuf,
    /// Already in its target folder: only the AI can move it to a better subfolder.
    pub in_place: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub home: PathBuf,
    pub items: Vec<Item>,
    pub skipped_projects: Vec<PathBuf>,
    /// Files left out because the AI already sorted them.
    pub ai_sorted_skipped: usize,
}

/// Drops files the AI already placed, as long as they are still where it put them.
pub fn drop_ai_sorted(result: &mut ScanResult, ai_sorted: &HashSet<PathBuf>) {
    let before = result.items.len();
    result.items.retain(|i| !(i.in_place && ai_sorted.contains(&i.src)));
    result.ai_sorted_skipped = before - result.items.len();
}

/// Collects files to sort.
/// `roots`: folders or files picked by the user (empty = whole home); must live under `home`.
/// A picked folder is always scanned, even if its name is on the skip lists.
/// `only`: restrict to these lowercase extensions (None = all known).
/// `include_sorted`: also list files already in Pictures/Music/Movies/Documents (for AI re-sorting).
pub fn scan(
    layout: &Layout,
    roots: &[PathBuf],
    only: Option<&HashSet<String>>,
    include_sorted: bool,
) -> ScanResult {
    let home = &layout.home;
    let mut items = Vec::new();
    let mut skipped_projects = Vec::new();
    let mut seen = HashSet::new();
    let whole_home = [home.to_path_buf()];
    let roots = if roots.is_empty() { &whole_home[..] } else { roots };

    let mut push = |path: &Path, size: u64, items: &mut Vec<Item>| {
        if !seen.insert(path.to_path_buf()) {
            return;
        }
        if let Some(only) = only {
            match extension_of(path) {
                Some(ext) if only.contains(&ext) => {}
                _ => return,
            }
        }
        let Some(category) = category_for(path) else { return };
        let Some(dest) = destination(layout, path, category) else { return };
        // Already in place (e.g. ~/Pictures/a.jpg)
        let in_place = dest == path;
        if in_place && !include_sorted {
            return;
        }
        let target_root = layout.root(category);
        items.push(Item { src: path.to_path_buf(), dest, category, size, target_root, in_place });
    };

    for root in roots {
        if !root.starts_with(home) {
            continue;
        }
        if root.is_file() {
            let size = root.metadata().map(|m| m.len()).unwrap_or(0);
            push(root, size, &mut items);
            continue;
        }
        let is_home = root == home;
        let home_dev = fs::metadata(home).map(|m| m.dev()).unwrap_or(0);
        let walker = WalkDir::new(root)
            .min_depth(1)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                // Hidden files and Windows leftovers like $RECYCLE.BIN
                if name.starts_with('.') || name.starts_with('$') {
                    return false;
                }
                if e.file_type().is_dir() {
                    // Don't enter other volumes mounted in home (OrbStack, network shares...).
                    // walkdir's same_file_system() aborts the whole walk on such mounts.
                    if e.metadata().map(|m| m.dev() != home_dev).unwrap_or(true) {
                        return false;
                    }
                    if is_home && e.depth() == 1 && is_skipped_top_level(&name) {
                        return false;
                    }
                    if layout.is_sorted_root(e.path()) && !include_sorted {
                        return false;
                    }
                    if is_app_library(home, e.path()) {
                        return false;
                    }
                    // Packages, and assets of saved web pages ("Page_files" next to "Page.html")
                    if is_package(e.path()) || name.ends_with("_files") {
                        return false;
                    }
                    if is_project_dir(e.path()) {
                        skipped_projects.push(e.path().to_path_buf());
                        return false;
                    }
                }
                true
            });
        for entry in walker.filter_map(Result::ok) {
            if entry.file_type().is_file() {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                push(entry.path(), size, &mut items);
            }
        }
    }

    ScanResult { home: home.to_path_buf(), items, skipped_projects, ai_sorted_skipped: 0 }
}

/// Adds " (1)", " (2)"... until the path is free.
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or(Path::new(""));
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1..)
        .map(|i| parent.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        // EXDEV: different volume, fall back to copy + delete
        Err(e) if e.raw_os_error() == Some(18) => {
            fs::copy(from, to)?;
            // Never leave a stray copy if the original can't be removed (e.g. root-owned mount)
            fs::remove_file(from).inspect_err(|_| {
                let _ = fs::remove_file(to);
            })
        }
        Err(e) => Err(e),
    }
}

/// Deletes folders emptied by a move, up to (not including) the top-level folder of home.
/// A lone .DS_Store does not count as content.
fn remove_empty_parents(home: &Path, moved: &Path) {
    let mut dir = moved.parent();
    while let Some(d) = dir {
        let Ok(rel) = d.strip_prefix(home) else { break };
        if rel.components().count() < 2 {
            break;
        }
        let Ok(entries) = fs::read_dir(d) else { break };
        let names: Vec<_> = entries.filter_map(Result::ok).map(|e| e.file_name()).collect();
        if names.iter().any(|n| n != ".DS_Store") {
            break;
        }
        if names.is_empty() || fs::remove_file(d.join(".DS_Store")).is_ok() {
            if fs::remove_dir(d).is_err() {
                break;
            }
        }
        dir = d.parent();
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Move {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Cleans an AI-proposed subfolder: max 2 levels, no hidden/absolute/parent parts.
pub fn sanitize_subfolder(raw: &str) -> Option<PathBuf> {
    let parts: Vec<String> = raw
        .split('/')
        .map(|p| {
            p.chars()
                .filter(|c| !matches!(c, '\\' | ':' | '\0') && !c.is_control())
                .collect::<String>()
                .trim()
                .trim_start_matches('.')
                .chars()
                .take(60)
                .collect::<String>()
                .trim()
                .to_string()
        })
        .filter(|p| !p.is_empty())
        .take(2)
        .collect();
    (!parts.is_empty()).then(|| parts.iter().collect())
}

#[derive(Deserialize, Debug, Clone)]
pub struct MoveRequest {
    pub src: PathBuf,
    #[serde(default)]
    pub subfolder: Option<String>,
}

/// Destinations are recomputed here; the UI only picks sources and optional subfolders.
pub fn apply(layout: &Layout, requests: &[MoveRequest]) -> (Vec<Move>, Vec<String>) {
    let home = &layout.home;
    let mut moves = Vec::new();
    let mut errors = Vec::new();
    for req in requests {
        let src = &req.src;
        let sub = req.subfolder.as_deref().and_then(sanitize_subfolder);
        let dest = category_for(src).and_then(|c| match (&sub, destination(layout, src, c)) {
            (Some(sub), Some(_)) => Some(layout.root(c).join(sub).join(src.file_name()?)),
            (None, d) => d,
            (Some(_), None) => None,
        });
        let Some(dest) = dest else {
            errors.push(format!("Refusé : {}", src.display()));
            continue;
        };
        if !src.is_file() {
            errors.push(format!("Introuvable : {}", src.display()));
            continue;
        }
        // Already where it should be
        if dest == *src {
            continue;
        }
        let target = unique_path(&dest);
        match move_file(src, &target) {
            Ok(()) => {
                remove_empty_parents(home, src);
                moves.push(Move {
                    from: src.clone(),
                    to: target,
                })
            }
            Err(e) => errors.push(format!("{} : {e}", src.display())),
        }
    }
    (moves, errors)
}

pub fn undo(moves: &[Move]) -> (usize, Vec<String>) {
    let mut restored = 0;
    let mut errors = Vec::new();
    for m in moves.iter().rev() {
        if !m.to.is_file() || m.from.exists() {
            errors.push(format!("Ignoré : {}", m.to.display()));
            continue;
        }
        match move_file(&m.to, &m.from) {
            Ok(()) => restored += 1,
            Err(e) => errors.push(format!("{} : {e}", m.to.display())),
        }
    }
    (restored, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lay(h: &Path) -> Layout {
        Layout::for_home(h)
    }

    fn req(src: PathBuf, sub: Option<&str>) -> MoveRequest {
        MoveRequest { src, subfolder: sub.map(String::from) }
    }

    #[test]
    fn reads_localized_xdg_user_dirs() {
        let mut l = Layout::for_home(Path::new("/home/ana"));
        l.apply_user_dirs(
            "# written by xdg-user-dirs-update\n\
             XDG_DESKTOP_DIR=\"$HOME/Bureau\"\n\
             XDG_DOWNLOAD_DIR=\"$HOME/Téléchargements\"\n\
             XDG_PICTURES_DIR=\"$HOME/Images\"\n\
             XDG_VIDEOS_DIR=\"$HOME/Vidéos\"\n\
             XDG_MUSIC_DIR=\"$HOME/\"\n\
             XDG_DOCUMENTS_DIR=\"/etc/evil\"\n",
        );
        assert_eq!(l.root(Category::Pictures), PathBuf::from("/home/ana/Images"));
        assert_eq!(l.root(Category::Movies), PathBuf::from("/home/ana/Vidéos"));
        assert_eq!(l.root(Category::Installers), PathBuf::from("/home/ana/Téléchargements/Installeurs"));
        // $HOME itself = disabled, outside home = ignored: defaults stay
        assert_eq!(l.root(Category::Music), PathBuf::from("/home/ana/Music"));
        assert_eq!(l.root(Category::Documents), PathBuf::from("/home/ana/Documents"));
        assert_eq!(l.folder(Category::Archives), "Documents/Archives");
    }

    #[test]
    fn localized_target_folders_are_not_rescanned() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path();
        touch(&h.join("Images/old.jpg"));
        touch(&h.join("Téléchargements/new.jpg"));
        let mut l = Layout::for_home(h);
        l.apply_user_dirs("XDG_PICTURES_DIR=\"$HOME/Images\"\nXDG_DOWNLOAD_DIR=\"$HOME/Téléchargements\"\n");
        let res = scan(&l, &[], None, false);
        let got: Vec<_> = res.items.iter().map(|i| (i.src.strip_prefix(h).unwrap().to_path_buf(), i.dest.strip_prefix(h).unwrap().to_path_buf())).collect();
        assert_eq!(got, vec![(PathBuf::from("Téléchargements/new.jpg"), PathBuf::from("Images/new.jpg"))]);
    }

    #[test]
    fn sanitizes_subfolders() {
        assert_eq!(sanitize_subfolder("Factures"), Some(PathBuf::from("Factures")));
        assert_eq!(sanitize_subfolder("Albums/Artiste/Extra"), Some(PathBuf::from("Albums/Artiste")));
        assert_eq!(sanitize_subfolder("../../etc"), Some(PathBuf::from("etc")));
        assert_eq!(sanitize_subfolder("/.ssh"), Some(PathBuf::from("ssh")));
        assert_eq!(sanitize_subfolder("  "), None);
        assert_eq!(sanitize_subfolder(".."), None);
    }

    #[test]
    fn apply_uses_ai_subfolder_and_cannot_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path();
        touch(&h.join("Downloads/Trip/scan0012.pdf"));
        touch(&h.join("Downloads/x.pdf"));
        let (moves, errors) = apply(&lay(h), &[
            req(h.join("Downloads/Trip/scan0012.pdf"), Some("Factures")),
            req(h.join("Downloads/x.pdf"), Some("../../../tmp")),
        ]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(moves[0].to, h.join("Documents/Factures/scan0012.pdf"));
        assert_eq!(moves[1].to, h.join("Documents/tmp/x.pdf"));
    }

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    #[test]
    fn categorizes_by_extension_case_insensitive() {
        assert_eq!(category_for(Path::new("a.JPG")), Some(Category::Pictures));
        assert_eq!(category_for(Path::new("a.flac")), Some(Category::Music));
        assert_eq!(category_for(Path::new("a.mkv")), Some(Category::Movies));
        assert_eq!(category_for(Path::new("a.pdf")), Some(Category::Documents));
        assert_eq!(category_for(Path::new("a.ZIP")), Some(Category::Archives));
        assert_eq!(category_for(Path::new("a.dmg")), Some(Category::Installers));
        assert_eq!(category_for(Path::new("a.srt")), Some(Category::Movies));
        assert_eq!(category_for(Path::new("main.ts")), None);
        assert_eq!(category_for(Path::new("data.json")), None);
                assert_eq!(category_for(Path::new("README")), None);
    }

    #[test]
    fn destination_keeps_subfolders_but_drops_top_folder() {
        let h = Path::new("/h");
        assert_eq!(
            destination(&lay(h), Path::new("/h/Downloads/Trip/a.jpg"), Category::Pictures),
            Some(PathBuf::from("/h/Pictures/Trip/a.jpg"))
        );
        assert_eq!(
            destination(&lay(h), Path::new("/h/a.mp3"), Category::Music),
            Some(PathBuf::from("/h/Music/a.mp3"))
        );
        assert_eq!(destination(&lay(h), Path::new("/other/a.mp3"), Category::Music), None);
        assert_eq!(
            destination(&lay(h), Path::new("/h/Desktop/Old/site.zip"), Category::Archives),
            Some(PathBuf::from("/h/Documents/Archives/Old/site.zip"))
        );
        assert_eq!(
            destination(&lay(h), Path::new("/h/Downloads/app.dmg"), Category::Installers),
            Some(PathBuf::from("/h/Downloads/Installeurs/app.dmg"))
        );
        // Already in place: no "Installeurs/Installeurs"
        assert_eq!(
            destination(&lay(h), Path::new("/h/Downloads/Installeurs/app.dmg"), Category::Installers),
            Some(PathBuf::from("/h/Downloads/Installeurs/app.dmg"))
        );
        assert_eq!(destination(&lay(h), Path::new("/h/x/../a.mp3"), Category::Music), None);
    }

    #[test]
    fn scan_skips_protected_places() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path();
        touch(&h.join("Downloads/photo.jpg"));
        touch(&h.join("Desktop/Trip/song.mp3"));
        touch(&h.join("Downloads/setup.dmg"));
        touch(&h.join("Downloads/.hidden.png"));
        touch(&h.join("Library/x.png"));
        touch(&h.join("Pictures/already.png"));
        touch(&h.join("Labs/repo/.git/HEAD"));
        touch(&h.join("Labs/repo/logo.png"));
        touch(&h.join("Desktop/My.app/icon.png"));
        touch(&h.join("OrbStack/disk.png"));
        touch(&h.join("fvm/versions/logo.png"));
        touch(&h.join("go/pkg/readme.md"));
        touch(&h.join("miniconda3/doc.pdf"));
        touch(&h.join("PycharmProjects/x/shot.png"));
        touch(&h.join("Downloads/$RECYCLE.BIN/old.jpg"));
        touch(&h.join("video/.idea/workspace.xml"));
        touch(&h.join("Audio/Page_files/logo.png"));

        let res = scan(&lay(h), &[], None, false);
        let mut srcs: Vec<_> = res
            .items
            .iter()
            .map(|i| i.src.strip_prefix(h).unwrap().to_path_buf())
            .collect();
        srcs.sort();
        assert_eq!(
            srcs,
            vec![
                PathBuf::from("Desktop/Trip/song.mp3"),
                PathBuf::from("Downloads/photo.jpg"),
                PathBuf::from("Downloads/setup.dmg"),
            ]
        );
        let mut projects = res.skipped_projects.clone();
        projects.sort();
        assert_eq!(projects, vec![h.join("Labs/repo")]);
    }

    #[test]
    fn scan_picked_roots_only() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path();
        touch(&h.join("video/Images/frame.png"));
        touch(&h.join("video/.idea/workspace.xml"));
        touch(&h.join("go/notes.pdf"));
        touch(&h.join("Downloads/a.jpg"));
        touch(&h.join("Desktop/b.mp3"));
        touch(&h.join("Pictures/already.jpg"));
        let roots = vec![
            h.join("go"),              // skipped at top level, but picked explicitly
            h.join("Desktop/b.mp3"),   // single file
            h.join("video"),
            h.join("Pictures/already.jpg"), // already in place
        ];
        let mut srcs: Vec<_> = scan(&lay(h), &roots, None, false)
            .items
            .iter()
            .map(|i| i.src.strip_prefix(h).unwrap().to_path_buf())
            .collect();
        srcs.sort();
        assert_eq!(
            srcs,
            vec![
                PathBuf::from("Desktop/b.mp3"),
                PathBuf::from("go/notes.pdf"),
                PathBuf::from("video/Images/frame.png"),
            ]
        );
    }

    #[test]
    fn rescan_sorted_folders_for_ai() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path();
        touch(&h.join("Documents/scan0012.pdf"));
        touch(&h.join("Documents/Factures/edf.pdf"));
        touch(&h.join("Pictures/Photo Booth Library/Pictures/me.jpg"));
        touch(&h.join("Music/Music/Media/song.mp3"));
        touch(&h.join("Movies/TV/show.mp4"));
        touch(&h.join("Documents/cover.png"));

        let res = scan(&lay(h), &[], None, true);
        let mut got: Vec<_> = res
            .items
            .iter()
            .map(|i| (i.src.strip_prefix(h).unwrap().to_path_buf(), i.in_place))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                (PathBuf::from("Documents/Factures/edf.pdf"), true),
                (PathBuf::from("Documents/cover.png"), false),
                (PathBuf::from("Documents/scan0012.pdf"), true),
            ]
        );

        // AI re-sort: move into a new subfolder, keep the one already fine, clean empty dirs
        let (moves, errors) = apply(&lay(h), &[
            req(h.join("Documents/scan0012.pdf"), Some("Administratif")),
            req(h.join("Documents/Factures/edf.pdf"), Some("Factures")),
        ]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(moves.len(), 1);
        assert!(h.join("Documents/Administratif/scan0012.pdf").is_file());
        assert!(h.join("Documents/Factures/edf.pdf").is_file());
    }

    #[test]
    fn ai_sorted_files_are_not_rescanned() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path();
        touch(&h.join("Documents/Factures/edf.pdf"));
        touch(&h.join("Documents/scan.pdf"));
        touch(&h.join("Downloads/new.pdf"));
        let ai_sorted: HashSet<PathBuf> = [
            h.join("Documents/Factures/edf.pdf"),
            // Registered but since moved by the user to Downloads: must come back
            h.join("Documents/Old/new.pdf"),
        ]
        .into();
        let mut res = scan(&lay(h), &[], None, true);
        drop_ai_sorted(&mut res, &ai_sorted);
        let mut names: Vec<_> = res.items.iter().map(|i| i.src.strip_prefix(h).unwrap().to_path_buf()).collect();
        names.sort();
        assert_eq!(names, vec![PathBuf::from("Documents/scan.pdf"), PathBuf::from("Downloads/new.pdf")]);
        assert_eq!(res.ai_sorted_skipped, 1);
    }

    #[test]
    fn removes_folders_emptied_by_a_move() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path();
        touch(&h.join("Downloads/Trip/day1/a.jpg"));
        touch(&h.join("Downloads/Trip/day1/.DS_Store"));
        touch(&h.join("Downloads/Trip/notes.txt"));
        let (moves, _) = apply(&lay(h), &[req(h.join("Downloads/Trip/day1/a.jpg"), None)]);
        assert_eq!(moves.len(), 1);
        assert!(!h.join("Downloads/Trip/day1").exists());
        assert!(h.join("Downloads/Trip/notes.txt").is_file());
        assert!(h.join("Downloads").is_dir());
        // Undo recreates the folder
        let (restored, _) = undo(&moves);
        assert_eq!(restored, 1);
        assert!(h.join("Downloads/Trip/day1/a.jpg").is_file());
    }

    #[test]
    fn scan_respects_extension_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path();
        touch(&h.join("Downloads/a.pdf"));
        touch(&h.join("Downloads/b.DOCX"));
        touch(&h.join("Downloads/c.jpg"));
        let only: HashSet<String> = ["pdf".to_string(), "docx".to_string()].into();
        let mut names: Vec<_> = scan(&lay(h), &[], Some(&only), false)
            .items
            .iter()
            .map(|i| i.src.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["a.pdf", "b.DOCX"]);
    }

    #[test]
    fn apply_renames_on_conflict_and_undo_restores() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path();
        touch(&h.join("Downloads/photo.jpg"));
        touch(&h.join("Desktop/Trip/song.mp3"));
        touch(&h.join("Pictures/photo.jpg"));

        let (moves, errors) =
            apply(&lay(h), &[req(h.join("Downloads/photo.jpg"), None), req(h.join("Desktop/Trip/song.mp3"), None)]);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(h.join("Pictures/photo (1).jpg").is_file());
        assert!(h.join("Music/Trip/song.mp3").is_file());
        assert!(!h.join("Downloads/photo.jpg").exists());

        let (restored, errors) = undo(&moves);
        assert_eq!(restored, 2);
        assert!(errors.is_empty());
        assert!(h.join("Downloads/photo.jpg").is_file());
        assert!(h.join("Desktop/Trip/song.mp3").is_file());
        assert!(h.join("Pictures/photo.jpg").is_file());
    }

    #[test]
    fn apply_refuses_paths_outside_home() {
        let tmp = tempfile::tempdir().unwrap();
        let (moves, errors) = apply(&lay(tmp.path()), &[req(PathBuf::from("/etc/hosts.txt"), None)]);
        assert!(moves.is_empty());
        assert_eq!(errors.len(), 1);
    }
}

/// Read-only preview on the real home: `cargo test --lib real_home_preview -- --ignored --nocapture`
#[cfg(test)]
mod preview {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    #[ignore]
    fn real_home_preview() {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let res = scan(&Layout::for_home(&home), &[], None, std::env::var("SORTED").is_ok());
        let mut by_dir: BTreeMap<String, (usize, u64)> = BTreeMap::new();
        for i in &res.items {
            let rel = i.src.strip_prefix(&home).unwrap();
            let mut c = rel.components();
            let top = c.next().unwrap().as_os_str().to_string_lossy().into_owned();
            let key = match (rel.components().count() > 2, c.next()) {
                (true, Some(second)) => format!("{top}/{}", second.as_os_str().to_string_lossy()),
                _ => top,
            };
            let e = by_dir.entry(key).or_default();
            e.0 += 1;
            e.1 += i.size;
        }
        println!("TOTAL {} files", res.items.len());
        for (d, (n, s)) in &by_dir {
            println!("{n:>6}  {:>8.1} MB  {d}", *s as f64 / 1e6);
        }
        println!("PROJECTS SKIPPED: {}", res.skipped_projects.len());
        for p in res.skipped_projects.iter().take(40) {
            println!("  {}", p.strip_prefix(&home).unwrap().display());
        }
    }
}
