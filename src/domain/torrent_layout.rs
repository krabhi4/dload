use std::path::{Path, PathBuf};

/// Shape of the files inside a torrent, as determined from its metadata.
///
/// Used to decide where on disk the content ends up and what to report as
/// `content_path` in the qBittorrent-compat API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentLayout {
    /// Exactly one file. The PathBuf is its path relative to the torrent's output folder
    /// (typically just a filename, but some .torrent files include subdirectory components).
    SingleFile(PathBuf),
    /// Multiple files that all share the same top-level directory. The String is that
    /// directory's name.
    MultiFileWithRoot(String),
    /// Multiple files with no shared top-level directory. librqbit would write them
    /// directly into `output_folder`, colliding with siblings. Caller must wrap into a
    /// torrent-named subfolder.
    MultiFileFlat,
}

/// Classify the layout from a collection of relative paths.
/// Padding-file entries should be filtered out by the caller before invoking this.
pub fn classify_layout<I, P>(relative_paths: I) -> ContentLayout
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let paths: Vec<PathBuf> = relative_paths
        .into_iter()
        .map(|p| p.as_ref().to_path_buf())
        .collect();

    if paths.len() == 1 {
        return ContentLayout::SingleFile(paths.into_iter().next().unwrap());
    }

    // Find each path's top-level directory. If a path has no directory component
    // (file at the root), it has no shared root with anything else.
    let first_dirs: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            let mut comps = p.components();
            let first = comps.next()?;
            if comps.next().is_some() {
                Some(first.as_os_str().to_string_lossy().to_string())
            } else {
                None
            }
        })
        .collect();

    if !first_dirs.is_empty()
        && first_dirs.len() == paths.len()
        && first_dirs.iter().all(|d| d == &first_dirs[0])
    {
        ContentLayout::MultiFileWithRoot(first_dirs.into_iter().next().unwrap())
    } else {
        ContentLayout::MultiFileFlat
    }
}

/// Paths to use once the layout is known.
///
/// `output_folder` is what to pass to librqbit (`AddTorrentOptions::output_folder`).
/// `content_path` is what to store on `Download` and return in the qBittorrent API —
/// always a strict child of the caller-provided `download_folder`, never equal to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentPaths {
    pub output_folder: String,
    pub content_path: String,
}

/// Compute final paths for a torrent given its download folder, torrent name (from metadata),
/// and layout. All returned paths use forward-slash separators.
pub fn compute_torrent_paths(
    download_folder: &str,
    torrent_name: &str,
    layout: &ContentLayout,
) -> TorrentPaths {
    let df = normalize(download_folder.trim_end_matches('/'));

    match layout {
        ContentLayout::SingleFile(rel) => {
            let rel_str = normalize(&rel.to_string_lossy());
            TorrentPaths {
                output_folder: df.clone(),
                content_path: join(&df, &rel_str),
            }
        }
        ContentLayout::MultiFileWithRoot(root) => {
            let root = normalize(root);
            TorrentPaths {
                output_folder: df.clone(),
                content_path: join(&df, &root),
            }
        }
        ContentLayout::MultiFileFlat => {
            // Wrap in a torrent-named subfolder so librqbit's flat write doesn't spray
            // files into the shared download folder (where *arr would then try to import
            // unrelated siblings).
            let wrapper = crate::domain::sanitize_filename(torrent_name);
            let wrapped = join(&df, &wrapper);
            TorrentPaths {
                output_folder: wrapped.clone(),
                content_path: wrapped,
            }
        }
    }
}

fn normalize(s: &str) -> String {
    s.replace('\\', "/")
}

fn join(base: &str, rel: &str) -> String {
    let base = base.trim_end_matches('/');
    let rel = rel.trim_start_matches('/');
    format!("{}/{}", base, rel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn pb(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    // ─── classify_layout ─────────────────────────────────────────────────

    #[test]
    fn classify_single_file() {
        let layout = classify_layout(vec![pb("movie.mkv")]);
        assert_eq!(layout, ContentLayout::SingleFile(pb("movie.mkv")));
    }

    #[test]
    fn classify_single_file_nested() {
        // Rare but valid: single-file torrent whose path has subdirs.
        let layout = classify_layout(vec![pb("sub/dir/movie.mkv")]);
        assert_eq!(layout, ContentLayout::SingleFile(pb("sub/dir/movie.mkv")));
    }

    #[test]
    fn classify_multi_with_common_root() {
        let layout = classify_layout(vec![
            pb("Show.S01/ep01.mkv"),
            pb("Show.S01/ep02.mkv"),
            pb("Show.S01/subs/ep01.srt"),
        ]);
        assert_eq!(layout, ContentLayout::MultiFileWithRoot("Show.S01".into()));
    }

    #[test]
    fn classify_multi_flat_no_root() {
        // Files directly at the top — no shared parent.
        let layout = classify_layout(vec![pb("a.mkv"), pb("b.mkv")]);
        assert_eq!(layout, ContentLayout::MultiFileFlat);
    }

    #[test]
    fn classify_multi_divergent_roots() {
        let layout = classify_layout(vec![pb("rootA/a.mkv"), pb("rootB/b.mkv")]);
        assert_eq!(layout, ContentLayout::MultiFileFlat);
    }

    #[test]
    fn classify_multi_one_at_root_one_nested() {
        // Mixed: one file has a parent dir, another doesn't. Treated as flat.
        let layout = classify_layout(vec![pb("a.mkv"), pb("sub/b.mkv")]);
        assert_eq!(layout, ContentLayout::MultiFileFlat);
    }

    // ─── compute_torrent_paths ───────────────────────────────────────────

    #[test]
    fn paths_single_file() {
        let layout = ContentLayout::SingleFile(pb("movie.mkv"));
        let p = compute_torrent_paths("/downloads", "ignored", &layout);
        assert_eq!(p.output_folder, "/downloads");
        assert_eq!(p.content_path, "/downloads/movie.mkv");
    }

    #[test]
    fn paths_single_file_strips_trailing_slash() {
        let layout = ContentLayout::SingleFile(pb("movie.mkv"));
        let p = compute_torrent_paths("/downloads/", "ignored", &layout);
        assert_eq!(p.output_folder, "/downloads");
        assert_eq!(p.content_path, "/downloads/movie.mkv");
    }

    #[test]
    fn paths_multi_with_root() {
        let layout = ContentLayout::MultiFileWithRoot("Show.S01".into());
        let p = compute_torrent_paths("/downloads", "ignored", &layout);
        assert_eq!(p.output_folder, "/downloads");
        assert_eq!(p.content_path, "/downloads/Show.S01");
    }

    #[test]
    fn paths_multi_flat_wraps_into_torrent_name() {
        let layout = ContentLayout::MultiFileFlat;
        let p = compute_torrent_paths("/downloads", "My.Release.2024", &layout);
        // librqbit's output_folder gets bumped so files land under the wrapper.
        assert_eq!(p.output_folder, "/downloads/My.Release.2024");
        // content_path == output_folder here: content IS the wrapper.
        assert_eq!(p.content_path, "/downloads/My.Release.2024");
    }

    #[test]
    fn paths_multi_flat_sanitizes_torrent_name() {
        let layout = ContentLayout::MultiFileFlat;
        let p = compute_torrent_paths("/downloads", "../../etc/passwd", &layout);
        // sanitize_filename strips path traversal, so the wrapper is just "passwd".
        assert_eq!(p.output_folder, "/downloads/passwd");
    }

    #[test]
    fn paths_normalize_backslashes() {
        // Windows-authored .torrent with backslashes in the relative path.
        let layout = ContentLayout::SingleFile(PathBuf::from("sub\\movie.mkv"));
        let p = compute_torrent_paths("C:\\downloads", "ignored", &layout);
        assert!(!p.content_path.contains('\\'), "got {:?}", p.content_path);
        assert!(!p.output_folder.contains('\\'), "got {:?}", p.output_folder);
    }

    #[test]
    fn paths_content_path_never_equals_download_folder() {
        // The critical *arr-compat invariant: content_path != save_path. Since the API
        // returns download_folder as save_path, content_path must not equal download_folder.
        for (name, layout) in [
            ("single", ContentLayout::SingleFile(pb("a.mkv"))),
            ("multi-root", ContentLayout::MultiFileWithRoot("root".into())),
            ("multi-flat", ContentLayout::MultiFileFlat),
        ] {
            let p = compute_torrent_paths("/downloads", "torrent-name", &layout);
            assert_ne!(
                p.content_path.trim_end_matches('/'),
                "/downloads",
                "layout {} violated invariant: content_path == download_folder",
                name
            );
        }
    }
}
