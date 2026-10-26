//! Exclusion rules: what is never, or not by default, synchronized.
//!
//! Evaluation order (first decisive answer wins):
//! 1. always-excluded (`.git/`, `.devbridge/`) — cannot be overridden;
//! 2. secret patterns, minus the user's explicit opt-ins;
//! 3. default exclusions followed by the user's own rules (a user `!pattern`
//!    can re-include a default exclusion);
//! 4. the project's `.gitignore` files (nested ones included), if enabled.
//!
//! The sender *and* the receiver evaluate these rules, so a peer can never
//! write a file that this device's rules exclude.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use devbridge_core::RelPath;
use devbridge_core::rules::{ALWAYS_EXCLUDED, DEFAULT_EXCLUDES, DEFAULT_SECRET_PATTERNS, SECRET_EXCEPTIONS};
use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exclusion {
    Always,
    Secret,
    Default,
    User,
    Gitignore,
}

#[derive(Debug, Clone, Default)]
pub struct RuleConfig {
    pub user_patterns: Vec<String>,
    pub secret_optins: Vec<String>,
    pub respect_gitignore: bool,
}

pub struct Rules {
    root: PathBuf,
    always: Gitignore,
    secret: Gitignore,
    general: Gitignore,
    user_lines: Vec<String>,
    respect_gitignore: bool,
    /// Directory (relative, "" for root) → parsed `.gitignore`, if any.
    gitignores: Mutex<HashMap<String, Option<Gitignore>>>,
}

fn build(root: &Path, lines: impl IntoIterator<Item = String>) -> Gitignore {
    let mut b = GitignoreBuilder::new(root);
    for l in lines {
        // Invalid user globs are skipped rather than failing the whole rule set.
        if let Err(e) = b.add_line(None, &l) {
            tracing::warn!(pattern = %l, error = %e, "ignoring invalid exclusion pattern");
        }
    }
    b.build().unwrap_or_else(|_| Gitignore::empty())
}

impl Rules {
    pub fn new(root: &Path, cfg: &RuleConfig) -> Self {
        let secret_lines = DEFAULT_SECRET_PATTERNS
            .iter()
            .filter(|p| !cfg.secret_optins.iter().any(|o| o == *p))
            .chain(SECRET_EXCEPTIONS)
            .map(|s| s.to_string());
        let user_lines: Vec<String> = cfg
            .user_patterns
            .iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty() && !s.starts_with('#'))
            .collect();
        Self {
            root: root.to_path_buf(),
            always: build(root, ALWAYS_EXCLUDED.iter().map(|s| s.to_string())),
            secret: build(root, secret_lines),
            general: build(root, DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).chain(user_lines.iter().cloned())),
            user_lines,
            respect_gitignore: cfg.respect_gitignore,
            gitignores: Mutex::new(HashMap::new()),
        }
    }

    /// Call when any `.gitignore` in the workspace changes.
    pub fn invalidate_gitignores(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Option<Gitignore>>> {
        self.gitignores.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn is_excluded(&self, rel: &RelPath, is_dir: bool) -> Option<Exclusion> {
        let p = Path::new(rel.as_str());
        if self.always.matched_path_or_any_parents(p, is_dir).is_ignore() {
            return Some(Exclusion::Always);
        }
        if self.secret.matched_path_or_any_parents(p, is_dir).is_ignore() {
            return Some(Exclusion::Secret);
        }
        match self.general.matched_path_or_any_parents(p, is_dir) {
            Match::Ignore(g) => {
                let user = self.user_lines.iter().any(|l| l == g.original());
                return Some(if user { Exclusion::User } else { Exclusion::Default });
            }
            // An explicit user re-include also overrides .gitignore.
            Match::Whitelist(_) => return None,
            Match::None => {}
        }
        if self.respect_gitignore && self.gitignored(rel, is_dir) {
            return Some(Exclusion::Gitignore);
        }
        None
    }

    fn gitignored(&self, rel: &RelPath, is_dir: bool) -> bool {
        // Directories containing `rel`, deepest first: "a/b", "a", "".
        let segs: Vec<&str> = rel.segments().collect();
        for depth in (0..segs.len()).rev() {
            let dir = segs[..depth].join("/");
            let rest: PathBuf = segs[depth..].iter().collect();
            let mut cache = self.lock();
            let gi = cache.entry(dir.clone()).or_insert_with(|| {
                let dir_abs = if dir.is_empty() { self.root.clone() } else { self.root.join(&dir) };
                let file = dir_abs.join(".gitignore");
                file.is_file().then(|| {
                    let mut b = GitignoreBuilder::new(&dir_abs);
                    b.add(&file);
                    b.build().unwrap_or_else(|_| Gitignore::empty())
                })
            });
            if let Some(gi) = gi {
                match gi.matched_path_or_any_parents(&rest, is_dir) {
                    Match::Ignore(_) => return true,
                    Match::Whitelist(_) => return false,
                    Match::None => {}
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(p: &str) -> RelPath {
        RelPath::parse(p).unwrap()
    }

    fn rules(root: &Path, cfg: RuleConfig) -> Rules {
        Rules::new(root, &cfg)
    }

    #[test]
    fn always_and_defaults() {
        let d = tempfile::tempdir().unwrap();
        let x = rules(d.path(), RuleConfig::default());
        assert_eq!(x.is_excluded(&r(".git/config"), false), Some(Exclusion::Always));
        assert_eq!(x.is_excluded(&r(".git"), true), Some(Exclusion::Always));
        assert_eq!(x.is_excluded(&r("web/node_modules/react/index.js"), false), Some(Exclusion::Default));
        assert_eq!(x.is_excluded(&r(".DS_Store"), false), Some(Exclusion::Default));
        assert_eq!(x.is_excluded(&r("src/main.rs"), false), None);
    }

    #[test]
    fn secrets_need_explicit_opt_in() {
        let d = tempfile::tempdir().unwrap();
        let x = rules(d.path(), RuleConfig::default());
        assert_eq!(x.is_excluded(&r(".env"), false), Some(Exclusion::Secret));
        assert_eq!(x.is_excluded(&r("api/.env.local"), false), Some(Exclusion::Secret));
        assert_eq!(x.is_excluded(&r("certs/server.pem"), false), Some(Exclusion::Secret));
        assert_eq!(x.is_excluded(&r(".ssh/id_ed25519"), false), Some(Exclusion::Secret));
        assert_eq!(x.is_excluded(&r(".claude/settings.local.json"), false), Some(Exclusion::Secret));
        assert_eq!(x.is_excluded(&r(".claude/settings.json"), false), None);
        assert_eq!(x.is_excluded(&r(".env.example"), false), None, "templates are safe");

        let y = rules(d.path(), RuleConfig { secret_optins: vec![".env".into()], ..Default::default() });
        assert_eq!(y.is_excluded(&r(".env"), false), None);
        assert_eq!(y.is_excluded(&r(".env.local"), false), Some(Exclusion::Secret));
    }

    #[test]
    fn user_rules_and_reinclusion() {
        let d = tempfile::tempdir().unwrap();
        let x =
            rules(d.path(), RuleConfig { user_patterns: vec!["*.log".into(), "!dist/".into()], ..Default::default() });
        assert_eq!(x.is_excluded(&r("a/debug.log"), false), Some(Exclusion::User));
        assert_eq!(x.is_excluded(&r("dist/app.js"), false), None, "user re-included a default exclusion");
        // Users can't re-include secrets or .git by pattern.
        let y =
            rules(d.path(), RuleConfig { user_patterns: vec!["!.env".into(), "!.git/".into()], ..Default::default() });
        assert_eq!(y.is_excluded(&r(".env"), false), Some(Exclusion::Secret));
        assert_eq!(y.is_excluded(&r(".git/HEAD"), false), Some(Exclusion::Always));
    }

    #[test]
    fn nested_gitignore_semantics() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(".gitignore"), "*.gen\n/out/\n").unwrap();
        std::fs::create_dir_all(d.path().join("pkg")).unwrap();
        std::fs::write(d.path().join("pkg/.gitignore"), "!keep.gen\nlocal/\n").unwrap();
        let x = rules(d.path(), RuleConfig { respect_gitignore: true, ..Default::default() });
        assert_eq!(x.is_excluded(&r("a.gen"), false), Some(Exclusion::Gitignore));
        assert_eq!(x.is_excluded(&r("out/x.txt"), false), Some(Exclusion::Gitignore));
        assert_eq!(x.is_excluded(&r("pkg/out/x.txt"), false), None, "anchored to root");
        assert_eq!(x.is_excluded(&r("pkg/keep.gen"), false), None, "deeper whitelist wins");
        assert_eq!(x.is_excluded(&r("pkg/other.gen"), false), Some(Exclusion::Gitignore));
        assert_eq!(x.is_excluded(&r("pkg/local/a"), false), Some(Exclusion::Gitignore));

        let off = rules(d.path(), RuleConfig::default());
        assert_eq!(off.is_excluded(&r("a.gen"), false), None);

        std::fs::write(d.path().join(".gitignore"), "").unwrap();
        assert_eq!(x.is_excluded(&r("a.gen"), false), Some(Exclusion::Gitignore), "cached");
        x.invalidate_gitignores();
        assert_eq!(x.is_excluded(&r("a.gen"), false), None);
    }
}
