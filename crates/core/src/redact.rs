//! Removes credentials from text before it is logged, shown in diagnostics or
//! exported. Applied to Git output (remote URLs can embed tokens) and to every
//! log line written to disk.

use std::sync::LazyLock;

use regex::Regex;

static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        // scheme://user:secret@host → scheme://***@host
        (r"(?i)\b([a-z][a-z0-9+.-]*://)[^/\s@]+@", "${1}***@"),
        (r"\bgh[pousr]_[A-Za-z0-9]{20,}", "***"),
        (r"\bgithub_pat_[A-Za-z0-9_]{20,}", "***"),
        (r"\bglpat-[A-Za-z0-9_-]{16,}", "***"),
        (r"\bsk-[A-Za-z0-9_-]{16,}", "***"),
        (r"\bxox[abprs]-[A-Za-z0-9-]{10,}", "***"),
        (r"\bAKIA[0-9A-Z]{16}\b", "***"),
        (r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}", "${1} ***"),
        (r#"(?i)\b(password|passwd|pwd|token|secret|api[_-]?key|access[_-]?key)(\s*[=:]\s*)[^\s&"']+"#, "${1}${2}***"),
    ]
    .into_iter()
    .map(|(p, r)| (Regex::new(p).expect("static regex"), r))
    .collect()
});

pub fn redact(s: &str) -> String {
    let mut out = s.to_owned();
    for (re, rep) in PATTERNS.iter() {
        if re.is_match(&out) {
            out = re.replace_all(&out, *rep).into_owned();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_common_secrets() {
        assert_eq!(
            redact("https://nick:ghp_abcdefghijklmnopqrstuv1234@github.com/x.git"),
            "https://***@github.com/x.git"
        );
        assert_eq!(redact("token ghp_abcdefghijklmnopqrstuvwxyz used"), "token *** used");
        assert_eq!(redact("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.x"), "Authorization: Bearer ***");
        assert_eq!(redact("password=hunter22 next"), "password=*** next");
        assert_eq!(redact("key sk-ant-api03-abcdefghijklmnop"), "key ***");
        assert_eq!(redact("AKIAIOSFODNN7EXAMPLE"), "***");
    }

    #[test]
    fn leaves_ordinary_text() {
        let s = "fatal: repository 'https://github.com/a/b.git/' not found";
        assert_eq!(redact(s), s);
        assert_eq!(redact("git@github.com:a/b.git"), "git@github.com:a/b.git");
    }
}
