//! Context attached to an agent message. The user sees it before sending.
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attachment {
    File {
        path: PathBuf,
        content: String,
    },
    Selection {
        path: PathBuf,
        start_line: u32,
        content: String,
    },
    Output {
        label: String,
        content: String,
        file: Option<PathBuf>,
    },
    /// Sent as a real image: `localImage` for Codex, an image block for Claude.
    Image {
        path: PathBuf,
        name: String,
        media_type: String,
    },
    /// A PDF, as a document block. Only Claude's SDK has one.
    Document {
        path: PathBuf,
        name: String,
    },
}

/// What this agent's integration can take, shown before anything is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub image_bytes: u64,
    /// The official schema has a way to send a PDF.
    pub documents: bool,
    pub document_bytes: u64,
    /// The selected model takes images.
    pub images: bool,
}

/// Text goes into the message itself, so it stays small.
pub const TEXT_BYTES: u64 = 1024 * 1024;

impl Limits {
    pub fn hint(&self) -> String {
        let mut s = String::new();
        if self.images {
            s.push_str(&format!(
                "Images: PNG, JPEG, GIF, WebP up to {} MB. ",
                self.image_bytes / (1024 * 1024)
            ));
        }
        s.push_str(&format!("Text up to {} MB.", TEXT_BYTES / (1024 * 1024)));
        if self.documents {
            s.push_str(&format!(
                " PDF up to {} MB.",
                self.document_bytes / (1024 * 1024)
            ));
        }
        s
    }
}

fn image_media_type(head: &[u8]) -> Option<&'static str> {
    match head {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("image/webp"),
        _ => None,
    }
}

fn megabytes(n: u64) -> String {
    format!("{:.1} MB", n as f64 / (1024. * 1024.))
}

/// Decide from the file itself what can be attached, and say why not otherwise.
pub fn classify(path: &Path, limits: &Limits) -> Result<Attachment, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let meta = std::fs::metadata(path).map_err(|e| format!("{name}: {e}"))?;
    if meta.is_dir() {
        return Err(format!("{name} is a folder: attach the files inside it."));
    }
    let size = meta.len();
    // The header decides the type: a huge video must not be read to learn that.
    let mut head = [0u8; 16];
    let got = {
        use std::io::Read as _;
        std::fs::File::open(path)
            .and_then(|mut f| f.read(&mut head))
            .map_err(|e| format!("{name}: {e}"))?
    };
    let head = &head[..got];
    if let Some(media_type) = image_media_type(head) {
        if !limits.images {
            return Err(format!("{name}: the selected model does not take images."));
        }
        if size > limits.image_bytes {
            return Err(format!(
                "{name} is {}. The limit for images here is {}.",
                megabytes(size),
                megabytes(limits.image_bytes)
            ));
        }
        return Ok(Attachment::Image {
            path: path.to_path_buf(),
            name,
            media_type: media_type.to_string(),
        });
    }
    if head.starts_with(b"%PDF") {
        if !limits.documents {
            return Err(format!("{name}: this agent has no way to receive a PDF."));
        }
        if size > limits.document_bytes {
            return Err(format!(
                "{name} is {}. The limit for PDF here is {}.",
                megabytes(size),
                megabytes(limits.document_bytes)
            ));
        }
        return Ok(Attachment::Document {
            path: path.to_path_buf(),
            name,
        });
    }
    let unsupported = || {
        format!(
            "{name}: this format cannot be sent. Supported: {}",
            limits.hint()
        )
    };
    if head.contains(&0) {
        return Err(unsupported());
    }
    if size > TEXT_BYTES {
        return Err(format!(
            "{name} is {}. Files up to {} can be attached as text.",
            megabytes(size),
            megabytes(TEXT_BYTES)
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    match std::str::from_utf8(&bytes) {
        Ok(content) if !bytes.contains(&0) => Ok(Attachment::File {
            path: path.to_path_buf(),
            content: content.to_string(),
        }),
        _ => Err(unsupported()),
    }
}

impl Attachment {
    pub fn title(&self, root: &Path) -> String {
        match self {
            // A file from elsewhere shows its name: the full path says nothing here.
            Attachment::File { path, .. } if !path.starts_with(root) => path
                .file_name()
                .map_or_else(|| rel(path, root), |n| n.to_string_lossy().to_string()),
            Attachment::File { path, .. } => rel(path, root),
            Attachment::Selection {
                path,
                start_line,
                content,
            } => {
                let end = start_line + content.lines().count().saturating_sub(1) as u32;
                format!("{}:{start_line}-{end}", rel(path, root))
            }
            Attachment::Output { label, .. } => label.clone(),
            Attachment::Image { name, .. } | Attachment::Document { name, .. } => name.clone(),
        }
    }

    pub fn image_path(&self) -> Option<&Path> {
        match self {
            Attachment::Image { path, .. } => Some(path),
            _ => None,
        }
    }

    /// The file behind this attachment, for a preview.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Attachment::File { path, .. }
            | Attachment::Selection { path, .. }
            | Attachment::Image { path, .. }
            | Attachment::Document { path, .. } => Some(path),
            Attachment::Output { .. } => None,
        }
    }

    pub fn render(&self, root: &Path) -> String {
        match self {
            Attachment::File { path, content } => {
                format!("<file path=\"{}\">\n{content}\n</file>", rel(path, root))
            }
            Attachment::Selection {
                path,
                start_line,
                content,
            } => format!(
                "<selection path=\"{}\" start_line=\"{start_line}\">\n{content}\n</selection>",
                rel(path, root)
            ),
            Attachment::Output {
                label,
                content,
                file,
            } => {
                let f = file
                    .as_ref()
                    .map(|p| format!(" related_file=\"{}\"", rel(p, root)))
                    .unwrap_or_default();
                format!("<output label=\"{label}\"{f}>\n{content}\n</output>")
            }
            // Never by name in the text: these go as real content blocks.
            Attachment::Image { .. } | Attachment::Document { .. } => String::new(),
        }
    }
}

fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Files that must never be attached automatically (a user can still paste by hand).
pub fn is_secret_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    if name == ".env" || name.starts_with(".env.") || name.ends_with(".env") {
        return !matches!(
            name.as_str(),
            ".env.example" | ".env.sample" | ".env.template"
        );
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if matches!(
        ext.as_str(),
        "pem" | "key" | "p12" | "pfx" | "keystore" | "jks" | "asc" | "gpg"
    ) {
        return true;
    }
    if matches!(
        name.as_str(),
        "id_rsa"
            | "id_ed25519"
            | "id_ecdsa"
            | "id_dsa"
            | ".npmrc"
            | ".netrc"
            | ".pypirc"
            | "credentials"
            | "credentials.json"
            | "secrets.json"
            | "secrets.yaml"
            | "secrets.yml"
            | "auth.json"
            | "service-account.json"
    ) {
        return true;
    }
    path.components()
        .any(|c| matches!(c.as_os_str().to_str(), Some(".ssh" | ".aws" | ".gnupg")))
}

/// Mask values that look like credentials before terminal text leaves the IDE.
/// Returns the text and how many values were masked.
pub fn scrub_secrets(text: &str) -> (String, usize) {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r#"(?ix)
            \b(?:sk-[a-z0-9_\-]{16,}|gh[pousr]_[a-z0-9]{20,}|github_pat_[a-z0-9_]{20,}
               |AKIA[0-9A-Z]{16}|xox[baprs]-[a-z0-9\-]{10,}|AIza[0-9a-z_\-]{30,})\b
            |(?P<name>\b[a-z0-9_]*(?:api_?key|secret|token|password|passwd)[a-z0-9_]*\s*[=:]\s*)
             (?P<value>"[^"\n]+"|'[^'\n]+'|[^\s"']{4,})
            "#,
        )
        .expect("valid regex")
    });
    let mut count = 0;
    let out = re.replace_all(text, |c: &regex::Captures<'_>| {
        count += 1;
        match c.name("name") {
            Some(n) => format!("{}***", n.as_str()),
            None => "***".to_string(),
        }
    });
    (out.into_owned(), count)
}

/// Compose the text message from user text and attachments.
pub fn compose_message(text: &str, attachments: &[Attachment], root: &Path) -> String {
    let mut out = String::new();
    for a in attachments {
        let rendered = a.render(root);
        if !rendered.is_empty() {
            out.push_str(&rendered);
            out.push_str("\n\n");
        }
    }
    out.push_str(text);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_files_are_recognized() {
        for p in [
            ".env",
            ".env.local",
            "prod.env",
            "server.pem",
            "a/b/id_rsa",
            ".npmrc",
            "/h/.ssh/config",
            "x.key",
            "auth.json",
        ] {
            assert!(is_secret_file(Path::new(p)), "{p}");
        }
        for p in [
            ".env.example",
            "App.tsx",
            "environment.ts",
            "keys.ts",
            "README.md",
        ] {
            assert!(!is_secret_file(Path::new(p)), "{p}");
        }
    }

    const LIMITS: Limits = Limits {
        image_bytes: 100,
        documents: false,
        document_bytes: 0,
        images: true,
    };

    #[test]
    fn files_are_judged_by_their_content() {
        let d = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let p = d.path().join(name);
            std::fs::write(&p, bytes).unwrap();
            p
        };
        let png = write("named.txt", b"\x89PNG\r\n\x1a\nrest");
        assert!(matches!(
            classify(&png, &LIMITS),
            Ok(Attachment::Image { media_type, .. }) if media_type == "image/png"
        ));
        let jpg = write("a.jpg", b"\xFF\xD8\xFFrest");
        assert!(matches!(
            classify(&jpg, &LIMITS),
            Ok(Attachment::Image { media_type, .. }) if media_type == "image/jpeg"
        ));
        let code = write("a.rs", "fn main() {} // я".as_bytes());
        assert!(
            matches!(classify(&code, &LIMITS), Ok(Attachment::File { content, .. }) if content.contains('я'))
        );
        let big = write("big.png", &[0x89, b'P', b'N', b'G'].repeat(50));
        assert!(classify(&big, &LIMITS).unwrap_err().contains("limit"));
        let pdf = write("a.pdf", b"%PDF-1.7");
        assert!(classify(&pdf, &LIMITS).unwrap_err().contains("PDF"));
        let ok_pdf = classify(
            &pdf,
            &Limits {
                documents: true,
                document_bytes: 100,
                ..LIMITS
            },
        );
        assert!(matches!(ok_pdf, Ok(Attachment::Document { .. })));
        let bin = write("a.mp4", b"\x00\x00\x00\x18ftypmp42");
        assert!(
            classify(&bin, &LIMITS)
                .unwrap_err()
                .contains("cannot be sent")
        );
        assert!(classify(d.path(), &LIMITS).unwrap_err().contains("folder"));
        let no_vision = Limits {
            images: false,
            ..LIMITS
        };
        assert!(
            classify(&jpg, &no_vision)
                .unwrap_err()
                .contains("does not take images")
        );
    }

    #[test]
    fn a_visual_attachment_is_never_only_a_name_in_the_text() {
        let a = Attachment::Image {
            path: "/p/shot.png".into(),
            name: "shot.png".into(),
            media_type: "image/png".into(),
        };
        let m = compose_message("what is this?", &[a], Path::new("/p"));
        assert_eq!(m, "what is this?");
    }

    #[test]
    fn terminal_text_loses_anything_key_like() {
        let (t, n) = scrub_secrets(
            "ok\nexport OPENAI_API_KEY=abcd1234\nkey sk-ant-api03-abcdefghijklmnop\nPASSWORD: \"hunter22\"\nghp_abcdefghijklmnopqrstuvwxyz\nnormal = text",
        );
        assert_eq!(n, 4);
        assert!(t.contains("OPENAI_API_KEY=***"));
        assert!(!t.contains("abcd1234"));
        assert!(!t.contains("sk-ant"));
        assert!(!t.contains("hunter22"));
        assert!(!t.contains("ghp_"));
        assert!(t.contains("normal = text"));
    }

    #[test]
    fn compose_includes_every_attachment_before_text() {
        let root = Path::new("/p");
        let atts = vec![
            Attachment::Selection {
                path: "/p/src/a.ts".into(),
                start_line: 3,
                content: "x\ny".into(),
            },
            Attachment::Output {
                label: "npm run dev".into(),
                content: "boom".into(),
                file: Some("/p/src/a.ts".into()),
            },
        ];
        assert_eq!(atts[0].title(root), "src/a.ts:3-4");
        let m = compose_message("fix", &atts, root);
        assert!(m.contains("<selection path=\"src/a.ts\" start_line=\"3\">"));
        assert!(m.contains("related_file=\"src/a.ts\""));
        assert!(m.ends_with("fix"));
    }
}
