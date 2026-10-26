//! Version ancestry and the decision of what to do with an incoming change.
//!
//! Every file version records a bounded list of ancestors (nearest first). An
//! incoming version fast-forwards the local one only if the local version is
//! among its ancestors. Timestamps never decide anything.

use serde::{Deserialize, Serialize};

/// Maximum ancestors kept per version. Beyond this, a stale-but-unrelated local
/// version is (safely) reported as a conflict rather than overwritten.
pub const MAX_ANCESTRY: usize = 64;

/// Maximum distinct ancestor content keys carried with a version.
pub const MAX_BASES: usize = 16;

/// Short content key of a version: the first 16 hex characters (64 bits) of
/// its content hash, or `-` for a deletion.
pub fn content_key(hash: Option<&str>, deleted: bool) -> String {
    if deleted { "-".to_owned() } else { hash.map(|h| h.chars().take(16).collect()).unwrap_or_default() }
}

/// The local device's current version of one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalHead {
    pub version_id: String,
    pub content_hash: Option<String>,
    pub deleted: bool,
}

/// What the receiver knows about an incoming version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingVersion<'a> {
    pub version_id: &'a str,
    pub ancestry: &'a [String],
    pub content_hash: Option<&'a str>,
    pub deleted: bool,
    /// Content keys of the incoming version's ancestors (see [`content_key`]).
    pub base_hashes: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Already have this version (redelivery, or we're ahead of it).
    Duplicate,
    /// Local version is an ancestor (or absent): safe to apply.
    FastForward,
    /// Different histories but identical content: no file change needed.
    /// Both sides adopt the larger version id as head.
    Converge { head: String },
    /// Independent changes: keep both, ask the user.
    Conflict,
}

/// `known` must be true if the incoming version id is already in local history.
pub fn classify(local: Option<&LocalHead>, incoming: &IncomingVersion<'_>, known: bool) -> Decision {
    if known {
        return Decision::Duplicate;
    }
    let Some(local) = local else { return Decision::FastForward };
    if local.version_id == incoming.version_id {
        return Decision::Duplicate;
    }
    if incoming.ancestry.contains(&local.version_id) {
        return Decision::FastForward;
    }
    let same_content = match (local.deleted, incoming.deleted) {
        (true, true) => true,
        (false, false) => local.content_hash.is_some() && local.content_hash.as_deref() == incoming.content_hash,
        _ => false,
    };
    if same_content {
        let head = std::cmp::max(local.version_id.as_str(), incoming.version_id).to_owned();
        return Decision::Converge { head };
    }
    // The incoming version was derived from content identical to what we have
    // now (e.g. both devices started from the same files and recorded separate
    // versions of them): applying it loses nothing.
    let local_key = content_key(local.content_hash.as_deref(), local.deleted);
    if !local_key.is_empty() && incoming.base_hashes.contains(&local_key) {
        return Decision::FastForward;
    }
    Decision::Conflict
}

/// Ancestor content keys for a new version with the given parents
/// (`(parent content key, parent's own base keys)`), deduplicated and bounded.
pub fn child_bases(parents: &[(String, &[String])]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (k, _) in parents {
        if !k.is_empty() && !out.contains(k) {
            out.push(k.clone());
        }
    }
    for (_, bases) in parents {
        for b in bases.iter() {
            if !out.contains(b) {
                out.push(b.clone());
            }
        }
    }
    out.truncate(MAX_BASES);
    out
}

/// Ancestry for a new version whose parents are `parents` (nearest first).
/// Each parent contributes itself followed by its own ancestry; duplicates are
/// removed and the result is truncated to [`MAX_ANCESTRY`].
pub fn child_ancestry(parents: &[(&str, &[String])]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |v: &str, out: &mut Vec<String>| {
        if !out.iter().any(|x| x == v) {
            out.push(v.to_owned());
        }
    };
    for (id, _) in parents {
        push(id, &mut out);
    }
    // Interleave ancestries so neither parent's recent history is truncated away.
    let longest = parents.iter().map(|(_, a)| a.len()).max().unwrap_or(0);
    for i in 0..longest {
        for (_, anc) in parents {
            if let Some(v) = anc.get(i) {
                push(v, &mut out);
            }
        }
    }
    out.truncate(MAX_ANCESTRY);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(v: &str, h: Option<&str>, deleted: bool) -> LocalHead {
        LocalHead { version_id: v.into(), content_hash: h.map(Into::into), deleted }
    }

    fn inc<'a>(v: &'a str, anc: &'a [String], h: Option<&'a str>, deleted: bool) -> IncomingVersion<'a> {
        IncomingVersion { version_id: v, ancestry: anc, content_hash: h, deleted, base_hashes: &[] }
    }

    #[test]
    fn new_path_fast_forwards() {
        assert_eq!(classify(None, &inc("v1", &[], Some("h"), false), false), Decision::FastForward);
    }

    #[test]
    fn known_is_duplicate() {
        let l = head("v2", Some("h2"), false);
        assert_eq!(classify(Some(&l), &inc("v1", &[], Some("h1"), false), true), Decision::Duplicate);
        assert_eq!(classify(Some(&l), &inc("v2", &[], Some("h2"), false), false), Decision::Duplicate);
    }

    #[test]
    fn descendant_fast_forwards_even_through_gaps() {
        let l = head("v1", Some("h1"), false);
        let anc = vec!["v3".to_string(), "v2".into(), "v1".into()];
        assert_eq!(classify(Some(&l), &inc("v4", &anc, Some("h4"), false), false), Decision::FastForward);
    }

    #[test]
    fn concurrent_edits_conflict_regardless_of_order() {
        let l = head("v2a", Some("ha"), false);
        let anc = vec!["v1".to_string()];
        assert_eq!(classify(Some(&l), &inc("v2b", &anc, Some("hb"), false), false), Decision::Conflict);
    }

    #[test]
    fn identical_concurrent_edits_converge_symmetrically() {
        let anc = vec!["v1".to_string()];
        let a = classify(Some(&head("v2a", Some("h"), false)), &inc("v2b", &anc, Some("h"), false), false);
        let b = classify(Some(&head("v2b", Some("h"), false)), &inc("v2a", &anc, Some("h"), false), false);
        assert_eq!(a, Decision::Converge { head: "v2b".into() });
        assert_eq!(a, b, "both devices must pick the same head");
    }

    #[test]
    fn delete_vs_modify_conflicts_and_double_delete_converges() {
        let anc = vec!["v1".to_string()];
        assert_eq!(
            classify(Some(&head("v2a", None, true)), &inc("v2b", &anc, Some("h"), false), false),
            Decision::Conflict
        );
        assert_eq!(
            classify(Some(&head("v2a", Some("h"), false)), &inc("v2b", &anc, None, true), false),
            Decision::Conflict
        );
        assert!(matches!(
            classify(Some(&head("v2a", None, true)), &inc("v2b", &anc, None, true), false),
            Decision::Converge { .. }
        ));
    }

    #[test]
    fn derived_from_identical_content_fast_forwards() {
        // Both devices recorded separate versions of the same initial content
        // (key "aaaa..."); the mac then edited before convergence finished.
        let pc_head = head("v_pc", Some("aaaaaaaaaaaaaaaaffff"), false);
        let bases = vec![content_key(Some("aaaaaaaaaaaaaaaa0000"), false)];
        let edit = IncomingVersion {
            version_id: "v_mac2",
            ancestry: &["v_mac1".to_string()],
            content_hash: Some("bbbb"),
            deleted: false,
            base_hashes: &bases,
        };
        assert_eq!(classify(Some(&pc_head), &edit, false), Decision::FastForward);
        // But if the PC changed its copy since, it's a real conflict.
        let pc_edited = head("v_pc2", Some("cccccccccccccccc"), false);
        assert_eq!(classify(Some(&pc_edited), &edit, false), Decision::Conflict);
    }

    #[test]
    fn bases_are_deduplicated_and_bounded() {
        let many: Vec<String> = (0..40).map(|i| format!("{i:016}")).collect();
        let b = child_bases(&[("x".into(), &many), ("x".into(), &[])]);
        assert_eq!(b[0], "x");
        assert_eq!(b.len(), MAX_BASES);
        assert_eq!(content_key(Some("0123456789abcdef0123"), false), "0123456789abcdef");
        assert_eq!(content_key(None, true), "-");
    }

    #[test]
    fn merge_ancestry_contains_both_parents_and_is_bounded() {
        let a_anc: Vec<String> = (0..100).map(|i| format!("a{i}")).collect();
        let b_anc = vec!["b0".to_string()];
        let m = child_ancestry(&[("A", &a_anc), ("B", &b_anc)]);
        assert_eq!(&m[..4], &["A", "B", "a0", "b0"]);
        assert_eq!(m.len(), MAX_ANCESTRY);
    }
}
