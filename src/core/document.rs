//! On-disk state of an open file and the rules for external changes.
//! Nothing here overwrites a version without an explicit decision.
use super::textfile::{self, DecodeError, FileFormat, hash_bytes};
use anyhow::{Context as _, Result, anyhow};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskVersion {
    /// File contents at the given hash.
    Present(u64),
    Missing,
}

/// What must happen after the file changed outside of the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalAction {
    /// Disk equals what we already know.
    None,
    /// The buffer is clean: reload it with this text.
    Reload { text: String, format: FileFormat },
    /// Buffer has unsaved changes and the disk changed too: the user decides.
    Conflict {
        disk_text: String,
        disk_format: FileFormat,
    },
    /// File was deleted while the buffer is clean.
    DeletedClean,
    /// File was deleted while the buffer has unsaved changes.
    DeletedDirty,
    /// New contents cannot be shown as text.
    Unreadable(String),
}

#[derive(Debug, Clone)]
pub struct DocumentState {
    pub path: PathBuf,
    pub format: FileFormat,
    /// LF text that matches the last load or save.
    pub saved_text: String,
    /// Hash of the disk bytes the buffer is based on.
    pub base: DiskVersion,
    /// The user chose to keep their buffer over this disk version.
    pub conflict_with: Option<DiskVersion>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved,
    /// The disk changed since we last looked. Nothing was written.
    DiskChanged,
}

impl DocumentState {
    pub fn load(path: &Path) -> Result<(Self, String)> {
        let meta =
            std::fs::metadata(path).with_context(|| format!("cannot read {}", path.display()))?;
        if meta.len() > textfile::MAX_EDITABLE_BYTES {
            return Err(anyhow!(
                "file is too large to edit ({} MB)",
                meta.len() / 1024 / 1024
            ));
        }
        let bytes =
            std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        let decoded = textfile::decode(&bytes).map_err(|e: DecodeError| anyhow!("{e}"))?;
        let text = decoded.text.clone();
        Ok((
            Self {
                path: path.to_path_buf(),
                format: decoded.format,
                saved_text: decoded.text,
                base: DiskVersion::Present(hash_bytes(&bytes)),
                conflict_with: None,
            },
            text,
        ))
    }

    /// A buffer that is not backed by a file on disk yet.
    pub fn new_empty(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            format: FileFormat::default(),
            saved_text: String::new(),
            base: DiskVersion::Missing,
            conflict_with: None,
        }
    }

    pub fn is_dirty(&self, buffer: &str) -> bool {
        self.saved_text != buffer
    }

    fn disk_version(&self) -> Result<(DiskVersion, Option<Vec<u8>>)> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok((DiskVersion::Present(hash_bytes(&bytes)), Some(bytes))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((DiskVersion::Missing, None)),
            Err(e) => Err(e.into()),
        }
    }

    /// Compare the disk with our base version and decide what to do.
    pub fn check_external(&self, buffer: &str) -> Result<ExternalAction> {
        let (version, bytes) = self.disk_version()?;
        Ok(self.classify(version, bytes, buffer))
    }

    fn classify(
        &self,
        version: DiskVersion,
        bytes: Option<Vec<u8>>,
        buffer: &str,
    ) -> ExternalAction {
        let dirty = self.is_dirty(buffer);
        if version == self.base || self.conflict_with.as_ref() == Some(&version) {
            return ExternalAction::None;
        }
        let Some(bytes) = bytes else {
            return if dirty {
                ExternalAction::DeletedDirty
            } else {
                ExternalAction::DeletedClean
            };
        };
        match textfile::decode(&bytes) {
            Err(e) => ExternalAction::Unreadable(e.to_string()),
            // The disk now holds exactly the buffer: not a real conflict.
            Ok(d) if dirty && d.text != buffer => ExternalAction::Conflict {
                disk_text: d.text,
                disk_format: d.format,
            },
            Ok(d) => ExternalAction::Reload {
                text: d.text,
                format: d.format,
            },
        }
    }

    /// Record that the buffer was reloaded from the current disk version.
    pub fn accept_disk(&mut self, text: String, format: FileFormat) {
        self.base = match self.disk_version() {
            Ok((version, _)) => version,
            Err(_) => DiskVersion::Present(hash_bytes(&textfile::encode(&text, &format))),
        };
        self.saved_text = text;
        self.format = format;
        self.conflict_with = None;
    }

    /// The user keeps their buffer. Later saves still need `force`.
    pub fn keep_buffer(&mut self) -> Result<()> {
        self.conflict_with = Some(self.disk_version()?.0);
        Ok(())
    }

    /// Save unless the disk changed behind our back. `force` is used only after
    /// the user explicitly chose to overwrite the disk version.
    pub fn save(&mut self, buffer: &str, force: bool) -> Result<SaveOutcome> {
        let (version, _) = self.disk_version()?;
        if !force && version != self.base {
            return Ok(SaveOutcome::DiskChanged);
        }
        let bytes = textfile::encode(buffer, &self.format);
        atomic_write(&self.path, &bytes)?;
        self.base = DiskVersion::Present(hash_bytes(&bytes));
        self.saved_text = buffer.to_string();
        self.conflict_with = None;
        Ok(SaveOutcome::Saved)
    }
}

/// Write via a temporary file and rename, so that a crash cannot leave half a
/// file behind. Keeps permissions and writes through symlinks.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("no parent directory"))?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let tmp = dir.join(format!(".{name}.kern-{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        std::fs::write(&tmp, bytes)?;
        if let Ok(meta) = std::fs::metadata(&target) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, &target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.with_context(|| format!("cannot save {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(content: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Проект с пробелом.txt");
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[test]
    fn clean_buffer_reloads_on_external_change() {
        let (_d, path) = setup(b"one\n");
        let (doc, text) = DocumentState::load(&path).unwrap();
        std::fs::write(&path, b"two\n").unwrap();
        assert!(matches!(
            doc.check_external(&text).unwrap(),
            ExternalAction::Reload { text, .. } if text == "two\n"
        ));
    }

    #[test]
    fn dirty_buffer_conflicts_and_nothing_is_overwritten() {
        let (_d, path) = setup(b"one\n");
        let (mut doc, _) = DocumentState::load(&path).unwrap();
        std::fs::write(&path, b"disk\n").unwrap();
        let buffer = "mine\n";
        assert!(matches!(
            doc.check_external(buffer).unwrap(),
            ExternalAction::Conflict { disk_text, .. } if disk_text == "disk\n"
        ));
        // Plain save refuses and leaves the disk alone.
        assert_eq!(doc.save(buffer, false).unwrap(), SaveOutcome::DiskChanged);
        assert_eq!(std::fs::read(&path).unwrap(), b"disk\n");
        // Choosing "keep mine" stops re-prompting but still does not write.
        doc.keep_buffer().unwrap();
        assert_eq!(doc.check_external(buffer).unwrap(), ExternalAction::None);
        assert_eq!(doc.save(buffer, false).unwrap(), SaveOutcome::DiskChanged);
        assert_eq!(std::fs::read(&path).unwrap(), b"disk\n");
        // Explicit overwrite.
        assert_eq!(doc.save(buffer, true).unwrap(), SaveOutcome::Saved);
        assert_eq!(std::fs::read(&path).unwrap(), b"mine\n");
        assert!(!doc.is_dirty(buffer));
    }

    #[test]
    fn load_disk_resolves_conflict() {
        let (_d, path) = setup(b"one\n");
        let (mut doc, _) = DocumentState::load(&path).unwrap();
        std::fs::write(&path, b"disk\r\n").unwrap();
        let ExternalAction::Conflict {
            disk_text,
            disk_format,
        } = doc.check_external("mine").unwrap()
        else {
            panic!()
        };
        doc.accept_disk(disk_text.clone(), disk_format);
        assert_eq!(
            doc.check_external(&disk_text).unwrap(),
            ExternalAction::None
        );
        assert_eq!(doc.save(&disk_text, false).unwrap(), SaveOutcome::Saved);
        assert_eq!(std::fs::read(&path).unwrap(), b"disk\r\n");
    }

    #[test]
    fn disk_equal_to_buffer_is_not_a_conflict() {
        let (_d, path) = setup(b"one\n");
        let (doc, _) = DocumentState::load(&path).unwrap();
        std::fs::write(&path, b"same\n").unwrap();
        assert!(matches!(
            doc.check_external("same\n").unwrap(),
            ExternalAction::Reload { .. }
        ));
    }

    #[test]
    fn deleted_file_is_reported_and_kept_in_buffer() {
        let (_d, path) = setup(b"one\n");
        let (doc, text) = DocumentState::load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            doc.check_external(&text).unwrap(),
            ExternalAction::DeletedClean
        );
        assert_eq!(
            doc.check_external("edited").unwrap(),
            ExternalAction::DeletedDirty
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_keeps_bom_crlf_and_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, path) = setup(b"\xEF\xBB\xBFa\r\nb\r\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (mut doc, text) = DocumentState::load(&path).unwrap();
        assert_eq!(text, "a\nb\n");
        assert_eq!(doc.save("a\nb\nc\n", false).unwrap(), SaveOutcome::Saved);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"\xEF\xBB\xBFa\r\nb\r\nc\r\n"
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_writes_through_symlink() {
        let (d, path) = setup(b"x\n");
        let link = d.path().join("link.txt");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let (mut doc, _) = DocumentState::load(&link).unwrap();
        doc.save("y\n", false).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"y\n");
    }
}
