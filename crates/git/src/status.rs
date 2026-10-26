//! Parser for `git status --porcelain=v2 --branch -z`.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Ordinary,
    Renamed,
    Unmerged,
    Untracked,
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusEntry {
    pub path: String,
    pub orig_path: Option<String>,
    /// Index (staged) status letter, '.' when unchanged.
    pub index: char,
    /// Working-tree status letter, '.' when unchanged.
    pub worktree: char,
    pub kind: EntryKind,
}

impl StatusEntry {
    pub fn staged(&self) -> bool {
        matches!(self.kind, EntryKind::Ordinary | EntryKind::Renamed) && self.index != '.'
    }
    pub fn unstaged(&self) -> bool {
        matches!(self.kind, EntryKind::Ordinary | EntryKind::Renamed) && self.worktree != '.'
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub entries: Vec<StatusEntry>,
}

impl Status {
    pub fn is_clean(&self) -> bool {
        self.entries.iter().all(|e| e.kind == EntryKind::Ignored)
    }
    pub fn has_conflicts(&self) -> bool {
        self.entries.iter().any(|e| e.kind == EntryKind::Unmerged)
    }
}

fn xy(s: &str) -> (char, char) {
    let mut c = s.chars();
    (c.next().unwrap_or('.'), c.next().unwrap_or('.'))
}

pub fn parse(out: &str) -> Status {
    let mut st = Status::default();
    let mut recs = out.split('\0').filter(|s| !s.is_empty()).peekable();
    while let Some(rec) = recs.next() {
        if let Some(h) = rec.strip_prefix("# ") {
            let (k, v) = h.split_once(' ').unwrap_or((h, ""));
            match k {
                "branch.oid" if v != "(initial)" => st.head = Some(v.to_owned()),
                "branch.head" if v != "(detached)" => st.branch = Some(v.to_owned()),
                "branch.upstream" => st.upstream = Some(v.to_owned()),
                "branch.ab" => {
                    for part in v.split_whitespace() {
                        if let Some(a) = part.strip_prefix('+') {
                            st.ahead = a.parse().unwrap_or(0);
                        } else if let Some(b) = part.strip_prefix('-') {
                            st.behind = b.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let (tag, rest) = rec.split_at(1);
        let rest = rest.trim_start();
        match tag {
            "1" => {
                // XY sub mH mI mW hH hI path
                let f: Vec<&str> = rest.splitn(8, ' ').collect();
                if f.len() == 8 {
                    let (i, w) = xy(f[0]);
                    st.entries.push(StatusEntry {
                        path: f[7].into(),
                        orig_path: None,
                        index: i,
                        worktree: w,
                        kind: EntryKind::Ordinary,
                    });
                }
            }
            "2" => {
                // XY sub mH mI mW hH hI Xscore path \0 origPath
                let f: Vec<&str> = rest.splitn(9, ' ').collect();
                if f.len() == 9 {
                    let (i, w) = xy(f[0]);
                    let orig = recs.next().map(str::to_owned);
                    st.entries.push(StatusEntry {
                        path: f[8].into(),
                        orig_path: orig,
                        index: i,
                        worktree: w,
                        kind: EntryKind::Renamed,
                    });
                }
            }
            "u" => {
                // XY sub m1 m2 m3 mW h1 h2 h3 path
                let f: Vec<&str> = rest.splitn(10, ' ').collect();
                if f.len() == 10 {
                    let (i, w) = xy(f[0]);
                    st.entries.push(StatusEntry {
                        path: f[9].into(),
                        orig_path: None,
                        index: i,
                        worktree: w,
                        kind: EntryKind::Unmerged,
                    });
                }
            }
            "?" => st.entries.push(StatusEntry {
                path: rest.into(),
                orig_path: None,
                index: '?',
                worktree: '?',
                kind: EntryKind::Untracked,
            }),
            "!" => st.entries.push(StatusEntry {
                path: rest.into(),
                orig_path: None,
                index: '!',
                worktree: '!',
                kind: EntryKind::Ignored,
            }),
            _ => {}
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2() {
        let raw = "# branch.oid 1234abcd\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa aaa src/a.rs\0\
1 A. N... 000000 100644 100644 000 bbb new file.txt\0\
2 R. N... 100644 100644 100644 ccc ccc R100 b.rs\0old b.rs\0\
u UU N... 100644 100644 100644 100644 d1 d2 d3 conflict.rs\0\
? untracked.md\0";
        let s = parse(raw);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!((s.ahead, s.behind), (2, 1));
        assert_eq!(s.entries.len(), 5);
        assert!(s.entries[0].unstaged() && !s.entries[0].staged());
        assert_eq!(s.entries[1].path, "new file.txt");
        assert!(s.entries[1].staged());
        assert_eq!(s.entries[2].orig_path.as_deref(), Some("old b.rs"));
        assert!(s.has_conflicts());
        assert_eq!(s.entries[4].kind, EntryKind::Untracked);
        assert!(!s.is_clean());
    }

    #[test]
    fn initial_and_detached() {
        let s = parse("# branch.oid (initial)\0# branch.head (detached)\0");
        assert_eq!((s.head.as_deref(), s.branch.as_deref()), (None, None));
        assert!(s.is_clean());
    }
}
