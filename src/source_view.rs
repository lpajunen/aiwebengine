//! Where a build reads a script's files from.
//!
//! Until now there was one answer — the rows currently in the database — and
//! it was implicit in every call. That is what confines checking, testing and
//! running to whatever is deployed right now: a revision cannot be tested
//! before it is published, and a change spanning several modules cannot be
//! checked before every one of them has already replaced the module it is
//! superseding.
//!
//! A [`SourceView`] makes the answer a parameter. The three cases are the
//! three questions worth asking of a script's files:
//!
//! - [`SourceView::Live`] — what is deployed. Today's only behaviour.
//! - [`SourceView::Revision`] — what a stored revision held.
//! - [`SourceView::Overlay`] — a candidate that has not been stored at all,
//!   laid over one of the other two.
//!
//! The module loader threads one of these through the two places it reads
//! files, and its caches key on it, so a program built from a revision and the
//! program serving requests can be cached side by side without either
//! displacing the other.

use std::collections::BTreeMap;

use crate::repository;
use crate::revisions;

/// One file, as some view of a script holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    pub content: Vec<u8>,
    pub mimetype: String,
}

impl SourceFile {
    pub fn text(content: impl Into<String>, mimetype: impl Into<String>) -> Self {
        Self {
            content: content.into().into_bytes(),
            mimetype: mimetype.into(),
        }
    }
}

/// What an overlay says about one path.
///
/// A candidate change that *removes* a module is as much a change as one that
/// rewrites it, and a check that silently kept reading the deleted file would
/// pass on a program that cannot be built once the change lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayEntry {
    Written(SourceFile),
    Deleted,
}

/// Where a build reads a script's files from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SourceView {
    /// The rows currently stored — what requests are served from.
    #[default]
    Live,
    /// A stored revision of the script.
    Revision(i32),
    /// Files that exist only in this request, over a base view.
    ///
    /// Boxed because the base may itself be a revision: "this candidate, on
    /// top of what revision 40 held" is how a change is checked against the
    /// version it was written for rather than against whatever head has since
    /// become.
    Overlay {
        base: Box<SourceView>,
        files: BTreeMap<String, OverlayEntry>,
    },
}

impl SourceView {
    /// An overlay of `files` over the live rows.
    pub fn overlay(files: BTreeMap<String, OverlayEntry>) -> Self {
        SourceView::Overlay {
            base: Box::new(SourceView::Live),
            files,
        }
    }

    /// An overlay of `files` over `base`.
    pub fn overlay_on(base: SourceView, files: BTreeMap<String, OverlayEntry>) -> Self {
        SourceView::Overlay {
            base: Box::new(base),
            files,
        }
    }

    /// Whether this view is the deployed state.
    ///
    /// The caches that must be invalidated when a write lands hold entries for
    /// this view only; every other view describes content that cannot change
    /// under them.
    pub fn is_live(&self) -> bool {
        matches!(self, SourceView::Live)
    }

    /// A stable identity for cache keys.
    ///
    /// Distinct views must never share a key, and the same view must produce
    /// the same key every time — hence the digest over an overlay's sorted
    /// contents rather than anything derived from where it came from.
    pub fn cache_key(&self) -> String {
        match self {
            SourceView::Live => "live".to_string(),
            SourceView::Revision(revision) => format!("rev:{}", revision),
            SourceView::Overlay { base, files } => {
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(base.cache_key().as_bytes());
                for (path, entry) in files {
                    hasher.update(path.as_bytes());
                    hasher.update([0u8]);
                    match entry {
                        OverlayEntry::Written(file) => {
                            hasher.update(file.mimetype.as_bytes());
                            hasher.update([0u8]);
                            hasher.update(&file.content);
                        }
                        OverlayEntry::Deleted => hasher.update(b"\x01deleted"),
                    }
                    hasher.update([0u8]);
                }
                format!("ovl:{}", hex::encode(hasher.finalize()))
            }
        }
    }

    /// Which of the script's files is its root module under this view, or
    /// `None` when the view holds none of the root names.
    ///
    /// The root is one entry of the tree like every other, so the question is
    /// which entry — and the tree answers, in the order
    /// [`crate::module_loader::ROOT_MODULE_NAMES`] lists.
    ///
    /// `Live` asks the database for the one row rather than listing the tree,
    /// because this is on the build path and a script's tree is every bundle
    /// and image it owns.
    pub fn root_path(&self, script_uri: &str) -> Option<String> {
        let names = crate::module_loader::ROOT_MODULE_NAMES;
        match self {
            SourceView::Live => repository::find_root_asset(script_uri),
            SourceView::Revision(_) => {
                let held = self.list_paths(script_uri);
                names
                    .iter()
                    .find(|name| held.iter().any(|path| path == *name))
                    .map(|name| (*name).to_string())
            }
            SourceView::Overlay { base, files } => {
                // A root the overlay writes wins over one the base holds, and
                // a root the overlay *deletes* must not fall through to the
                // base — deleting the entrypoint is a candidate change like
                // any other, and a check that kept building against the old
                // one would pass on a program that cannot be built.
                for name in names {
                    match files.get(name) {
                        Some(OverlayEntry::Written(_)) => return Some(name.to_string()),
                        Some(OverlayEntry::Deleted) => continue,
                        None => {}
                    }
                }
                base.root_path(script_uri)
                    .filter(|path| !matches!(files.get(path), Some(OverlayEntry::Deleted)))
            }
        }
    }

    /// The script's root source under this view.
    ///
    /// One file of the tree, read the way every other one is.
    pub fn root_content(&self, script_uri: &str) -> Option<String> {
        let path = self.root_path(script_uri)?;
        let file = self.fetch(script_uri, &path)?;
        String::from_utf8(file.content).ok()
    }

    /// One of the script's files under this view, or `None` when the view does
    /// not contain it.
    pub fn fetch(&self, script_uri: &str, path: &str) -> Option<SourceFile> {
        match self {
            SourceView::Live => repository::fetch_asset(script_uri, path).map(|asset| SourceFile {
                content: asset.content,
                mimetype: asset.mimetype,
            }),
            SourceView::Revision(revision) => {
                match crate::database::run_blocking(revisions::read_file(
                    script_uri, *revision, path,
                )) {
                    Ok(found) => found.map(|(content, mimetype)| SourceFile { content, mimetype }),
                    Err(e) => {
                        tracing::warn!(
                            script = script_uri,
                            revision = *revision,
                            path = path,
                            "Failed reading revision file: {}",
                            e
                        );
                        None
                    }
                }
            }
            SourceView::Overlay { base, files } => match files.get(path) {
                Some(OverlayEntry::Written(file)) => Some(file.clone()),
                Some(OverlayEntry::Deleted) => None,
                None => base.fetch(script_uri, path),
            },
        }
    }

    /// Every file path the view contains, sorted.
    ///
    /// Used to discover test modules, which is why it lists paths rather than
    /// content: a script's assets include client bundles and images that a
    /// discovery pass has no reason to read.
    pub fn list_paths(&self, script_uri: &str) -> Vec<String> {
        match self {
            SourceView::Live => {
                let mut paths: Vec<String> =
                    repository::fetch_assets(script_uri).into_keys().collect();
                paths.sort();
                paths
            }
            SourceView::Revision(revision) => {
                match crate::database::run_blocking(revisions::files(script_uri, *revision)) {
                    Ok(Some(files)) => files.into_iter().map(|file| file.uri).collect(),
                    Ok(None) => Vec::new(),
                    Err(e) => {
                        tracing::warn!(
                            script = script_uri,
                            revision = *revision,
                            "Failed listing revision files: {}",
                            e
                        );
                        Vec::new()
                    }
                }
            }
            SourceView::Overlay { base, files } => {
                // A set rather than `Vec::contains` per candidate file: a
                // change of any size would otherwise cost a scan of the base
                // tree for each of its files.
                let mut paths: std::collections::BTreeSet<String> = base
                    .list_paths(script_uri)
                    .into_iter()
                    .filter(|path| !matches!(files.get(path), Some(OverlayEntry::Deleted)))
                    .collect();
                for (path, entry) in files {
                    if matches!(entry, OverlayEntry::Written(_)) {
                        paths.insert(path.clone());
                    }
                }
                paths.into_iter().collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay_files(entries: &[(&str, OverlayEntry)]) -> BTreeMap<String, OverlayEntry> {
        entries
            .iter()
            .map(|(path, entry)| ((*path).to_string(), entry.clone()))
            .collect()
    }

    #[test]
    fn live_and_revision_views_have_distinct_keys() {
        assert_eq!(SourceView::Live.cache_key(), "live");
        assert_eq!(SourceView::Revision(41).cache_key(), "rev:41");
        assert_ne!(
            SourceView::Revision(41).cache_key(),
            SourceView::Revision(42).cache_key()
        );
    }

    #[test]
    fn overlay_key_follows_content_not_construction() {
        let one = SourceView::overlay(overlay_files(&[(
            "server/a.ts",
            OverlayEntry::Written(SourceFile::text("export const a = 1;", "text/typescript")),
        )]));
        let same = SourceView::overlay(overlay_files(&[(
            "server/a.ts",
            OverlayEntry::Written(SourceFile::text("export const a = 1;", "text/typescript")),
        )]));
        let different = SourceView::overlay(overlay_files(&[(
            "server/a.ts",
            OverlayEntry::Written(SourceFile::text("export const a = 2;", "text/typescript")),
        )]));

        assert_eq!(one.cache_key(), same.cache_key());
        assert_ne!(one.cache_key(), different.cache_key());
    }

    #[test]
    fn overlay_key_separates_a_deletion_from_an_absence() {
        let deleted = SourceView::overlay(overlay_files(&[("server/a.ts", OverlayEntry::Deleted)]));
        let empty = SourceView::overlay(BTreeMap::new());

        assert_ne!(deleted.cache_key(), empty.cache_key());
    }

    #[test]
    fn overlay_key_distinguishes_its_base() {
        let files = overlay_files(&[(
            "server/a.ts",
            OverlayEntry::Written(SourceFile::text("export const a = 1;", "text/typescript")),
        )]);
        let over_live = SourceView::overlay(files.clone());
        let over_revision = SourceView::overlay_on(SourceView::Revision(40), files);

        assert_ne!(over_live.cache_key(), over_revision.cache_key());
    }

    #[test]
    fn only_the_live_view_reports_itself_as_live() {
        assert!(SourceView::Live.is_live());
        assert!(!SourceView::Revision(1).is_live());
        assert!(!SourceView::overlay(BTreeMap::new()).is_live());
    }
}
