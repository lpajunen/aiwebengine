//! Which of a script's files the world can reach, read off the tree.
//!
//! Exposure used to be a **side effect of an `init()` call**. A file became
//! world-readable because some line of `init()` named it, and three things
//! followed from that:
//!
//! - You could not look at a file and know whether it was public. You had to
//!   read `init()` and mentally execute it.
//! - The default was private and the failure mode was public. `try_serve_asset`
//!   applies no authorization at all, so a mistyped
//!   `registerAssetRoute("/config", "credentials.json")` published a file to
//!   the world, with nothing in the write path, the revision manifest or a git
//!   diff to show it.
//! - Script data — system prompts, skill definitions, few-shot examples, the
//!   most security-relevant category there is — was defined by the *absence*
//!   of a registration. It was private because nobody happened to name it.
//!
//! The directory says it instead:
//!
//! ```text
//! main.ts                 root module
//! lib/parse.ts            module, imported
//! skills/refund.md        script data, private
//! public/app.js           served, world-readable
//! resources/schema.json   MCP resource
//! ```
//!
//! **Why a convention and not an `exposure` column.** `git_sync` already made
//! this argument and refused a manifest: the directory structure already *is*
//! the mapping. A column would be invisible in a repository, unmappable in
//! both directions, and would reintroduce exactly the manifest that reasoning
//! rejected. A directory is visible in `ls`, in a pull request and in a
//! revision manifest, and it survives the round trip for free.
//!
//! Registration does not disappear. It stops carrying the security decision
//! and carries only what is cosmetic: an HTTP path that should not mirror the
//! file path, OpenAPI `summary`/`tags`, an MCP resource's
//! `name`/`description`/`mimeType`. Publishing a file then means **moving**
//! it, which is a reviewable act.
//!
//! ## What is enforced today
//!
//! Nothing. This reports. A deployment's scripts were written when a
//! registration was the whole of the decision, so enforcing the convention
//! would stop serving files that are being served right now — and the set of
//! them is only knowable from the live registries, which a script whose
//! `init()` failed never populated. [`report`] is how an operator sees the
//! blast radius on their own engine before any of it 404s; the registration
//! paths log a warning each time they publish a file from outside its
//! directory.

/// Where a file has to live to be served to the world.
pub const PUBLIC_DIR: &str = "public/";

/// Where a file has to live to be published as an MCP resource.
pub const RESOURCE_DIR: &str = "resources/";

/// What the tree says about one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exposure {
    /// Under `public/`: served to anyone who can reach the host.
    Public,
    /// Under `resources/`: readable by an MCP client that may reach the host.
    Resource,
    /// Everything else: reachable only to the linker and to the script
    /// itself. The default, and the one that used to have no positive marker.
    Private,
}

/// What the tree says about `path`.
///
/// A file under `resources/` is importable as well as publishable — the
/// directory governs *exposure*, not importability, so a schema that is both
/// imported and published picks `resources/` and is still linked. Saying so
/// explicitly because the opposite is the natural assumption.
pub fn of(path: &str) -> Exposure {
    if path.starts_with(PUBLIC_DIR) {
        Exposure::Public
    } else if path.starts_with(RESOURCE_DIR) {
        Exposure::Resource
    } else {
        Exposure::Private
    }
}

/// Whether `path` may be served over HTTP under the convention.
pub fn is_publishable(path: &str) -> bool {
    matches!(of(path), Exposure::Public)
}

/// Whether `path` may be published as an MCP resource under the convention.
pub fn is_resource(path: &str) -> bool {
    matches!(of(path), Exposure::Resource)
}

/// One registration that publishes a file the directory does not.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Misplaced {
    /// The HTTP path or resource URI it is published under.
    pub published_as: String,
    /// The file it publishes.
    pub path: String,
    /// Where the file would have to move to for this registration to survive
    /// enforcement.
    pub should_be: String,
}

/// What one script's exposure looks like.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScriptExposure {
    pub script_uri: String,
    /// Asset routes publishing a file from outside `public/`.
    pub routes: Vec<Misplaced>,
    /// MCP resources publishing a file from outside `resources/`.
    pub resources: Vec<Misplaced>,
    /// True when the script's `init()` **failed**, so what it would have
    /// published is not knowable from the registries.
    ///
    /// The honest answer rather than an empty list, because an empty list and
    /// "we could not look" are the two things an operator most needs to tell
    /// apart before enforcing anything.
    ///
    /// Deliberately not "did not initialize". A script with no `init()` at
    /// all never reaches `update_script_init_status`, so it sits at
    /// `initialized = false` for ever — and it is the *clearest* case there
    /// is, since a script that registers nothing publishes nothing. Reading
    /// the flag instead of the error reported almost every script on a real
    /// deployment as unclassifiable, which is the same as reporting nothing.
    pub unclassified: bool,
}

impl ScriptExposure {
    fn is_clean(&self) -> bool {
        self.routes.is_empty() && self.resources.is_empty() && !self.unclassified
    }
}

/// What enforcing the convention would change, across every script.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExposureReport {
    /// Only the scripts with something to say; a clean engine reports none.
    pub scripts: Vec<ScriptExposure>,
    /// How many registrations would stop working.
    pub would_break: usize,
    /// How many scripts could not be classified because their `init()` has
    /// not run cleanly.
    pub unclassified: usize,
}

/// Build the report from the live registries.
///
/// The live registries rather than the stored source, because a registration
/// is a *call* — what a script publishes is what its `init()` did, not what
/// its source appears to say. The cost is that a script whose `init()` failed
/// has registered nothing and cannot be classified, which is why that is
/// reported rather than counted as clean.
pub fn report(metadata: &[crate::repository::ScriptMetadata]) -> ExposureReport {
    use std::collections::BTreeMap;

    let mut by_script: BTreeMap<String, ScriptExposure> = metadata
        .iter()
        .map(|meta| {
            (
                meta.uri.clone(),
                ScriptExposure {
                    script_uri: meta.uri.clone(),
                    routes: Vec::new(),
                    resources: Vec::new(),
                    // A failed `init()` may have been part-way through
                    // registering, so what it publishes is unknown. One that
                    // ran cleanly, and one the script never had, are both
                    // knowable: the registries hold everything they did.
                    unclassified: meta.init_error.is_some(),
                },
            )
        })
        .collect();

    fn entry<'a>(
        map: &'a mut BTreeMap<String, ScriptExposure>,
        uri: &str,
    ) -> &'a mut ScriptExposure {
        map.entry(uri.to_string())
            .or_insert_with(|| ScriptExposure {
                script_uri: uri.to_string(),
                routes: Vec::new(),
                resources: Vec::new(),
                unclassified: false,
            })
    }

    for (path, registration) in crate::asset_registry::get_global_registry().get_all_registrations()
    {
        if is_publishable(&registration.asset_name) {
            continue;
        }
        entry(&mut by_script, &registration.script_uri)
            .routes
            .push(Misplaced {
                published_as: path,
                should_be: format!("{}{}", PUBLIC_DIR, registration.asset_name),
                path: registration.asset_name,
            });
    }

    for resource in crate::mcp::list_resources() {
        if is_resource(&resource.asset_name) {
            continue;
        }
        entry(&mut by_script, &resource.script_uri)
            .resources
            .push(Misplaced {
                published_as: resource.uri,
                should_be: format!("{}{}", RESOURCE_DIR, resource.asset_name),
                path: resource.asset_name,
            });
    }

    let scripts: Vec<ScriptExposure> = by_script
        .into_values()
        .filter(|script| !script.is_clean())
        .collect();

    ExposureReport {
        would_break: scripts
            .iter()
            .map(|script| script.routes.len() + script.resources.len())
            .sum(),
        unclassified: scripts.iter().filter(|script| script.unclassified).count(),
        scripts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directory_says_what_a_file_is() {
        assert_eq!(of("public/app.js"), Exposure::Public);
        assert_eq!(of("resources/schema.json"), Exposure::Resource);
        assert_eq!(of("skills/refund.md"), Exposure::Private);
        assert_eq!(of("lib/parse.ts"), Exposure::Private);
        assert_eq!(of("main.ts"), Exposure::Private);
    }

    /// The default is what the whole convention is for: a file is private
    /// because of where it is, not because nobody happened to name it.
    #[test]
    fn anything_unmarked_is_private() {
        assert_eq!(of("credentials.json"), Exposure::Private);
        assert_eq!(of(".env"), Exposure::Private);
        assert_eq!(of(""), Exposure::Private);
    }

    /// `public` and `publicity/` are different directories, and a prefix
    /// match that ignored the separator would publish the second.
    #[test]
    fn a_directory_is_a_directory_and_not_a_prefix() {
        assert_eq!(of("publicity/leak.txt"), Exposure::Private);
        assert_eq!(of("public"), Exposure::Private);
        assert_eq!(of("resourceful/x"), Exposure::Private);
        assert_eq!(of("not/public/app.js"), Exposure::Private);
    }
}
