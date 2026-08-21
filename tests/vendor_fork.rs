//! Guards the Textify-specific changes inside `vendor/gpui-component`.
//!
//! The vendored crate is a fork whose local changes are documented in
//! `vendor/gpui-component/TEXTIFY_FORK.md`. Re-vendoring upstream sources
//! deletes those changes together with the document that describes them, so a
//! markdown file alone cannot protect them. This test pins each documented
//! change to a distinctive source marker: if a re-vendor (or refactor) drops
//! one, the build fails here with a pointer to the fork document instead of
//! silently reintroducing the bugs the fork fixes.
//!
//! When a marker fails legitimately — because a fork change was deliberately
//! reworked — update the marker AND the corresponding entry in
//! `TEXTIFY_FORK.md` in the same commit.

use std::fs;
use std::path::{Path, PathBuf};

fn vendor_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/gpui-component")
}

fn vendor_source(relative: &str) -> String {
    let path = vendor_root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()))
}

#[track_caller]
fn assert_marker(source: &str, relative: &str, marker: &str, change: &str) {
    assert!(
        source.contains(marker),
        "vendor/gpui-component/{relative} no longer contains `{marker}`.\n\
         The Textify fork change \"{change}\" appears to be missing — it was\n\
         probably lost to an upstream re-vendor. See vendor/gpui-component/TEXTIFY_FORK.md\n\
         for the change and reapply it (or update this guard and the document together)."
    );
}

#[test]
fn fork_document_exists_and_lists_the_local_changes() {
    let document = vendor_source("TEXTIFY_FORK.md");
    for topic in [
        "tree-sitter-all-languages",
        "undo-memory budget",
        "Command-scroll",
        "zoom",
        "scroll_offset",
        "multicursor",
    ] {
        assert!(
            document.contains(topic),
            "TEXTIFY_FORK.md no longer documents \"{topic}\"; keep the fork \
             document in sync with this guard"
        );
    }
}

#[test]
fn language_set_is_reduced_with_the_full_set_behind_a_feature() {
    let manifest = vendor_source("Cargo.toml");
    assert_marker(
        &manifest,
        "Cargo.toml",
        "tree-sitter-all-languages",
        "tree-sitter-languages bundles only JSON and Markdown",
    );
    let languages = vendor_source("src/highlighter/languages.rs");
    assert_marker(
        &languages,
        "src/highlighter/languages.rs",
        "feature = \"tree-sitter-all-languages\"",
        "tree-sitter-languages bundles only JSON and Markdown",
    );
    assert_marker(
        &languages,
        "src/highlighter/languages.rs",
        "_ => Self::Plain,",
        "unknown languages fall back to plain text without a parser",
    );
}

#[test]
fn editor_builders_expose_resource_budgets() {
    let state = vendor_source("src/input/state.rs");
    assert_marker(
        &state,
        "src/input/state.rs",
        "pub fn undo_max_bytes",
        "editor builders expose an approximate undo-memory budget",
    );
    assert_marker(
        &state,
        "src/input/state.rs",
        "pub fn search_max_matches",
        "editor builders expose a search-decoration limit",
    );
}

#[test]
fn command_scroll_is_left_to_the_host_application() {
    let state = vendor_source("src/input/state.rs");
    assert_marker(
        &state,
        "src/input/state.rs",
        "Textify reserves Command-scroll",
        "Command-scroll is passed to the host application for per-tab zoom",
    );
}

#[test]
fn zoom_anchor_api_and_scroll_accessor_exist() {
    let state = vendor_source("src/input/state.rs");
    assert_marker(
        &state,
        "src/input/state.rs",
        "pub fn preserve_zoom_anchor_at",
        "text zoom keeps the row under the pointer anchored",
    );
    assert_marker(
        &state,
        "src/input/state.rs",
        "pub fn scroll_offset",
        "InputState::scroll_offset is public for host-side tests",
    );
}

// The ordering below is the fix for the mid-document zoom flash: the visible
// line range is derived from the scroll offset, so a pending zoom anchor must
// move the offset before the range is computed. Applying it any later shapes
// lines for the stale offset and the anchored frame paints a blank gap.
#[test]
fn zoom_anchor_is_applied_before_the_visible_range_is_computed() {
    let element = vendor_source("src/input/element.rs");
    let apply = element.find("state.apply_zoom_anchor(");
    let visible_range = element.find("self.calculate_visible_range(&state");
    let apply = apply.unwrap_or_else(|| {
        panic!(
            "vendor/gpui-component/src/input/element.rs no longer applies the zoom \
             anchor in prepaint; the mid-document zoom flash fix was lost. See \
             vendor/gpui-component/TEXTIFY_FORK.md."
        )
    });
    let visible_range = visible_range.expect(
        "could not locate the visible-range computation in element.rs; \
         update this guard alongside the fork change",
    );
    assert!(
        apply < visible_range,
        "the zoom anchor must be applied to the scroll offset BEFORE the visible \
         line range is computed, or zooming mid-document paints a one-frame blank \
         flash. See vendor/gpui-component/TEXTIFY_FORK.md."
    );
}

// The patch is what makes the fork reapplyable during upstream upgrades
// (scripts/upgrade-gpui-component.sh); regenerate it whenever the vendored
// sources change.
#[test]
fn reapplyable_fork_patch_exists_and_covers_the_key_files() {
    let patch_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/patches/gpui-component-textify.patch");
    let patch = fs::read_to_string(&patch_path).unwrap_or_else(|error| {
        panic!(
            "could not read {}: {error}\nRegenerate it with \
             scripts/upgrade-gpui-component.sh regenerate-patch <baseline-sha>",
            patch_path.display()
        )
    });
    for file in [
        "b/Cargo.toml",
        "b/src/input/state.rs",
        "b/src/input/element.rs",
        "b/src/input/multicursor.rs",
        "b/src/highlighter/languages.rs",
    ] {
        assert!(
            patch.contains(file),
            "vendor/patches/gpui-component-textify.patch no longer covers {file}; \
             regenerate it with scripts/upgrade-gpui-component.sh regenerate-patch \
             <baseline-sha> after changing the vendored crate"
        );
    }
}

#[test]
fn multicursor_editing_support_exists() {
    let multicursor = vendor_source("src/input/multicursor.rs");
    assert_marker(
        &multicursor,
        "src/input/multicursor.rs",
        "rectangular_selections",
        "selection sets provide multicursor and rectangular selection",
    );
}
