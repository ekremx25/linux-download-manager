//! Category folders under the download library.
//!
//! This is the single source of truth for "which extension belongs where".
//! The frontend renders the category the backend stored, so these tables are
//! never duplicated in JavaScript.

use std::fs;
use std::path::{Path, PathBuf};

/// Root folder created inside the user's XDG download directory.
pub const LIBRARY_DIR: &str = "LDM";

/// Stable identifiers. These double as the sidebar filter keys.
pub const CATEGORIES: [&str; 8] = [
    "image", "music", "video", "apps", "document", "compressed", "iso", "other",
];

const IMAGE: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "tif", "svg", "ico", "avif", "heic",
    "heif", "raw",
];
const MUSIC: &[&str] = &[
    "mp3", "flac", "wav", "ogg", "opus", "m4a", "aac", "wma", "alac", "aiff", "mid", "midi",
];
const VIDEO: &[&str] = &[
    "mp4", "mkv", "webm", "avi", "mov", "m4v", "wmv", "flv", "mpeg", "mpg", "ts", "m2ts", "3gp",
];
const APPS: &[&str] = &[
    "exe", "msi", "appimage", "deb", "rpm", "flatpak", "flatpakref", "run", "bin", "sh", "apk",
];
const DOCUMENTS: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "txt", "rtf", "csv",
    "epub", "mobi", "md", "json", "xml",
];
const ARCHIVES: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "bz2", "xz", "zst", "tgz", "tbz", "tbz2", "txz",
];
const ISO: &[&str] = &["iso", "img"];

/// Suffixes that must win over the plain extension. `foo.pkg.tar.zst` is an
/// application bundle, not an archive, even though `zst` is listed as one.
const COMPOUND: &[(&str, &str)] = &[(".pkg.tar.zst", "apps")];

/// Folder name shown on disk for a category key.
pub fn folder_for(category: &str) -> &'static str {
    match category {
        "image" => "Images",
        "music" => "Music",
        "video" => "Videos",
        "apps" => "Applications",
        "document" => "Documents",
        "compressed" => "Archives",
        "iso" => "ISO",
        _ => "Other",
    }
}

/// Classifies by the real file name, case-insensitively. Query strings are
/// never involved: `download?id=123` classifies as `other`, while the same
/// request answering with `ubuntu-26.04.iso` classifies as `iso`.
pub fn classify(file_name: &str) -> &'static str {
    let lowered = file_name.to_ascii_lowercase();

    for (suffix, category) in COMPOUND {
        if lowered.ends_with(suffix) {
            return category;
        }
    }

    // A name without a dot has no extension, even though rsplit yields the
    // whole string in that case.
    let extension = if lowered.contains('.') {
        lowered.rsplit('.').next().unwrap_or("")
    } else {
        ""
    };

    let listed = |table: &[&str]| !extension.is_empty() && table.contains(&extension);
    if listed(IMAGE) {
        "image"
    } else if listed(MUSIC) {
        "music"
    } else if listed(VIDEO) {
        "video"
    } else if listed(APPS) {
        "apps"
    } else if listed(DOCUMENTS) {
        "document"
    } else if listed(ARCHIVES) {
        "compressed"
    } else if listed(ISO) {
        "iso"
    } else {
        "other"
    }
}

/// Strips every character that could let a server-supplied name escape the
/// library directory: path separators, traversal, control characters and the
/// characters Windows rejects. The result is always a single path component.
pub fn sanitize_file_name(value: &str, fallback: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| {
            if character.is_control() || "/\\:*?\"<>|".contains(character) {
                '_'
            } else {
                character
            }
        })
        .take(180)
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        fallback.to_string()
    } else {
        cleaned.to_string()
    }
}

pub fn library_root(base: &Path) -> PathBuf {
    base.join(LIBRARY_DIR)
}

pub fn category_dir(base: &Path, category: &str) -> PathBuf {
    library_root(base).join(folder_for(category))
}

/// Creates `LDM/` and every category folder, ignoring the ones that already
/// exist, and returns the library root.
pub fn ensure_library(base: &Path) -> Result<PathBuf, String> {
    let root = library_root(base);
    fs::create_dir_all(&root)
        .map_err(|error| format!("failed to create download library {}: {error}", root.display()))?;
    for category in CATEGORIES {
        let dir = root.join(folder_for(category));
        fs::create_dir_all(&dir).map_err(|error| {
            format!("failed to create category folder {}: {error}", dir.display())
        })?;
    }
    Ok(root)
}

/// Creates the folder a single download belongs in, so the engine never has
/// to race a missing directory against the first write.
pub fn ensure_category_dir(base: &Path, category: &str) -> Result<PathBuf, String> {
    let dir = category_dir(base, category);
    fs::create_dir_all(&dir)
        .map_err(|error| format!("failed to create category folder {}: {error}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_tables_pick_the_expected_folder() {
        assert_eq!(classify("test.jpg"), "image");
        assert_eq!(classify("song.mp3"), "music");
        assert_eq!(classify("movie.mkv"), "video");
        assert_eq!(classify("program.AppImage"), "apps");
        assert_eq!(classify("document.pdf"), "document");
        assert_eq!(classify("archive.7z"), "compressed");
        assert_eq!(classify("ubuntu.iso"), "iso");
        assert_eq!(classify("disk.img"), "iso");
        assert_eq!(classify("unknown.xyz"), "other");
    }

    #[test]
    fn uppercase_extensions_classify_the_same() {
        assert_eq!(classify("IMAGE.JPG"), "image");
        assert_eq!(classify("VIDEO.MKV"), "video");
        assert_eq!(classify("LINUX.ISO"), "iso");
        assert_eq!(classify("PROGRAM.APPIMAGE"), "apps");
        assert_eq!(classify("SONG.FLAC"), "music");
        assert_eq!(classify("DOC.PDF"), "document");
        assert_eq!(classify("ARCHIVE.ZIP"), "compressed");
    }

    #[test]
    fn names_without_a_real_extension_fall_back_to_other() {
        assert_eq!(classify("no-extension"), "other");
        assert_eq!(classify("download?id=123"), "other");
        assert_eq!(classify(".hidden"), "other");
        assert_eq!(classify("trailing."), "other");
    }

    #[test]
    fn compound_suffixes_win_over_the_plain_extension() {
        assert_eq!(classify("app.pkg.tar.zst"), "apps");
        assert_eq!(classify("archive.tar.gz"), "compressed");
    }

    #[test]
    fn every_category_resolves_to_a_folder() {
        for category in CATEGORIES {
            assert!(!folder_for(category).is_empty());
        }
        assert_eq!(folder_for("iso"), "ISO");
        assert_eq!(folder_for("nope"), "Other");
    }

    #[test]
    fn server_supplied_names_cannot_escape_the_library() {
        assert_eq!(sanitize_file_name("../../etc/passwd", "download.bin"), "_.._etc_passwd");
        assert_eq!(sanitize_file_name("..", "download.bin"), "download.bin");
        assert_eq!(sanitize_file_name("/", "download.bin"), "_");
        assert_eq!(sanitize_file_name("a\0b", "download.bin"), "a_b");
        assert_eq!(sanitize_file_name(" plain.iso ", "download.bin"), "plain.iso");
        // Whatever the server sends, the result is a single component.
        for hostile in ["../../x", "..\\..\\x", "/etc/shadow", "a/b/c"] {
            let safe = sanitize_file_name(hostile, "download.bin");
            assert!(!safe.contains('/'), "{safe} kept a separator");
            assert!(!safe.contains('\\'), "{safe} kept a backslash");
            assert_ne!(safe, "..");
        }
    }

    #[test]
    fn documents_land_under_the_matching_library_folder() {
        let base = Path::new("/home/tester/Downloads");
        for (name, folder) in [
            ("test.jpg", "Images"),
            ("song.mp3", "Music"),
            ("movie.mkv", "Videos"),
            ("program.AppImage", "Applications"),
            ("document.pdf", "Documents"),
            ("archive.7z", "Archives"),
            ("ubuntu.iso", "ISO"),
            ("unknown.xyz", "Other"),
        ] {
            let category = classify(name);
            let path = category_dir(base, category).join(name);
            assert_eq!(
                path,
                base.join("LDM").join(folder).join(name),
                "{name} landed in the wrong folder"
            );
            // Everything stays inside the library.
            assert!(path.starts_with(base.join("LDM")));
        }
    }

}
