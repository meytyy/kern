//! Parser for `git status --porcelain=v2 --branch -z`.
use anyhow::{Result, anyhow};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

impl FileState {
    pub fn letter(self) -> &'static str {
        match self {
            FileState::Modified => "M",
            FileState::Added => "A",
            FileState::Deleted => "D",
            FileState::Renamed => "R",
            FileState::Copied => "C",
            FileState::TypeChanged => "T",
            FileState::Untracked => "U",
            FileState::Conflicted => "!",
        }
    }

    fn from_code(c: u8) -> Option<Self> {
        Some(match c {
            b'M' => FileState::Modified,
            b'A' => FileState::Added,
            b'D' => FileState::Deleted,
            b'R' => FileState::Renamed,
            b'C' => FileState::Copied,
            b'T' => FileState::TypeChanged,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub orig_path: Option<String>,
    pub staged: Option<FileState>,
    pub unstaged: Option<FileState>,
    pub untracked: bool,
    /// Summary state for display.
    pub state: FileState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    pub branch: Option<String>,
    pub detached: bool,
    pub oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileChange>,
    pub merging: bool,
    pub rebasing: bool,
}

impl RepoStatus {
    pub fn conflicts(&self) -> usize {
        self.files
            .iter()
            .filter(|f| f.state == FileState::Conflicted)
            .count()
    }
    pub fn staged_count(&self) -> usize {
        self.files.iter().filter(|f| f.staged.is_some()).count()
    }
}

pub fn parse_porcelain_v2(bytes: &[u8]) -> Result<RepoStatus> {
    let text = String::from_utf8_lossy(bytes);
    let mut st = RepoStatus::default();
    let mut records = text.split('\0');
    while let Some(rec) = records.next() {
        if rec.is_empty() {
            continue;
        }
        let (kind, rest) = rec
            .split_once(' ')
            .ok_or_else(|| anyhow!("bad status record: {rec:?}"))?;
        match kind {
            "#" => parse_header(rest, &mut st),
            "1" => {
                // 1 XY sub mH mI mW hH hI path
                let f: Vec<&str> = rest.splitn(8, ' ').collect();
                if f.len() != 8 {
                    return Err(anyhow!("bad status record: {rec:?}"));
                }
                st.files.push(tracked(f[0], f[7].to_string(), None));
            }
            "2" => {
                // 2 XY sub mH mI mW hH hI Xscore path NUL origPath
                let f: Vec<&str> = rest.splitn(9, ' ').collect();
                if f.len() != 9 {
                    return Err(anyhow!("bad status record: {rec:?}"));
                }
                let orig = records
                    .next()
                    .filter(|o| !o.is_empty())
                    .ok_or_else(|| anyhow!("missing rename source"))?;
                st.files
                    .push(tracked(f[0], f[8].to_string(), Some(orig.to_string())));
            }
            "u" => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                let f: Vec<&str> = rest.splitn(10, ' ').collect();
                if f.len() != 10 {
                    return Err(anyhow!("bad status record: {rec:?}"));
                }
                st.files.push(FileChange {
                    path: f[9].to_string(),
                    orig_path: None,
                    staged: None,
                    unstaged: Some(FileState::Conflicted),
                    untracked: false,
                    state: FileState::Conflicted,
                });
            }
            "?" => st.files.push(FileChange {
                path: rest.to_string(),
                orig_path: None,
                staged: None,
                unstaged: None,
                untracked: true,
                state: FileState::Untracked,
            }),
            "!" => {}
            _ => return Err(anyhow!("unknown status record: {rec:?}")),
        }
    }
    Ok(st)
}

fn tracked(xy: &str, path: String, orig_path: Option<String>) -> FileChange {
    let b = xy.as_bytes();
    let staged = b.first().copied().and_then(FileState::from_code);
    let unstaged = b.get(1).copied().and_then(FileState::from_code);
    FileChange {
        path,
        orig_path,
        state: unstaged.or(staged).unwrap_or(FileState::Modified),
        staged,
        unstaged,
        untracked: false,
    }
}

fn parse_header(rest: &str, st: &mut RepoStatus) {
    let Some((key, value)) = rest.split_once(' ') else {
        return;
    };
    match key {
        "branch.oid" if value != "(initial)" => st.oid = Some(value.to_string()),
        "branch.head" => {
            if value == "(detached)" {
                st.detached = true;
            } else {
                st.branch = Some(value.to_string());
            }
        }
        "branch.upstream" => st.upstream = Some(value.to_string()),
        "branch.ab" => {
            for part in value.split(' ') {
                if let Some(n) = part.strip_prefix('+') {
                    st.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    st.behind = n.parse().unwrap_or(0);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_record_kinds() {
        let raw = concat!(
            "# branch.oid abc123\0",
            "# branch.head feat/x\0",
            "# branch.upstream origin/feat/x\0",
            "# branch.ab +2 -1\0",
            "1 .M N... 100644 100644 100644 aaa aaa src/a b.ts\0",
            "1 M. N... 100644 100644 100644 aaa bbb staged.ts\0",
            "1 MM N... 100644 100644 100644 aaa bbb both.ts\0",
            "1 A. N... 000000 100644 100644 000 bbb added.ts\0",
            "1 D. N... 100644 000000 000000 aaa 000 gone.ts\0",
            "2 R. N... 100644 100644 100644 aaa bbb R100 new name.ts\0old name.ts\0",
            "u UU N... 100644 100644 100644 100644 a b c clash file.ts\0",
            "? файл с пробелом.ts\0",
            "! ignored\0"
        );
        let st = parse_porcelain_v2(raw.as_bytes()).unwrap();
        assert_eq!(st.branch.as_deref(), Some("feat/x"));
        assert_eq!(st.upstream.as_deref(), Some("origin/feat/x"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        assert_eq!(st.oid.as_deref(), Some("abc123"));
        assert_eq!(st.files.len(), 8);
        let get = |p: &str| st.files.iter().find(|f| f.path == p).unwrap();
        assert_eq!(get("src/a b.ts").unstaged, Some(FileState::Modified));
        assert_eq!(get("src/a b.ts").staged, None);
        assert_eq!(get("staged.ts").staged, Some(FileState::Modified));
        assert!(get("both.ts").staged.is_some() && get("both.ts").unstaged.is_some());
        assert_eq!(get("added.ts").staged, Some(FileState::Added));
        assert_eq!(get("gone.ts").staged, Some(FileState::Deleted));
        assert_eq!(get("new name.ts").orig_path.as_deref(), Some("old name.ts"));
        assert_eq!(get("clash file.ts").state, FileState::Conflicted);
        assert!(get("файл с пробелом.ts").untracked);
        assert_eq!(st.conflicts(), 1);
    }

    #[test]
    fn initial_and_detached() {
        let st = parse_porcelain_v2(b"# branch.oid (initial)\0# branch.head main\0").unwrap();
        assert_eq!(st.oid, None);
        assert_eq!(st.branch.as_deref(), Some("main"));
        let st = parse_porcelain_v2(b"# branch.oid abc\0# branch.head (detached)\0").unwrap();
        assert!(st.detached && st.branch.is_none());
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert!(parse_porcelain_v2(b"1 .M short\0").is_err());
        assert!(parse_porcelain_v2(b"2 R. N... 1 1 1 a b R100 only\0").is_err());
        assert!(parse_porcelain_v2(b"zzz\0").is_err());
    }
}
