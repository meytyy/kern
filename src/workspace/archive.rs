//! Read-only look inside archives and binary files. Archives are only listed:
//! nothing is extracted to disk or run. Listing stops after a number of entries
//! and of decompressed bytes, so a huge or hostile archive cannot stall KERN.
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
}

impl ArchiveKind {
    pub fn label(self) -> &'static str {
        match self {
            ArchiveKind::Zip => "ZIP archive",
            ArchiveKind::Tar => "tar archive",
            ArchiveKind::TarGz => "gzip-compressed tar archive",
        }
    }
}

/// `.zip`, `.tar`, `.tar.gz`, `.tgz`, any case.
pub fn archive_kind(path: &Path) -> Option<ArchiveKind> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if name.ends_with(".zip") {
        Some(ArchiveKind::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(ArchiveKind::TarGz)
    } else if name.ends_with(".tar") {
        Some(ArchiveKind::Tar)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub dir: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// More entries exist than were read.
    pub truncated: bool,
    /// Sum of the listed files' sizes once extracted.
    pub unpacked: u64,
}

pub const MAX_ENTRIES: usize = 5_000;
/// tar.gz has to be decompressed to reach every header.
const MAX_SCAN: u64 = 1 << 30;

pub fn list(path: &Path, kind: ArchiveKind) -> Result<Listing, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("cannot open: {e}"))?;
    let mut out = Listing::default();
    match kind {
        ArchiveKind::Zip => {
            let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
                .map_err(|e| format!("not a readable ZIP archive: {e}"))?;
            for i in 0..zip.len() {
                if out.entries.len() >= MAX_ENTRIES {
                    out.truncated = true;
                    break;
                }
                let f = zip
                    .by_index_raw(i)
                    .map_err(|e| format!("damaged ZIP entry: {e}"))?;
                push(&mut out, f.name().to_string(), f.size(), f.is_dir());
            }
        }
        ArchiveKind::Tar => tar_entries(file, &mut out)?,
        ArchiveKind::TarGz => {
            let gz = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
            tar_entries(gz.take(MAX_SCAN), &mut out)?
        }
    }
    Ok(out)
}

fn push(out: &mut Listing, path: String, size: u64, dir: bool) {
    if !dir {
        out.unpacked += size;
    }
    out.entries.push(Entry { path, size, dir });
}

fn tar_entries(reader: impl Read, out: &mut Listing) -> Result<(), String> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|e| format!("not a readable tar archive: {e}"))?;
    for entry in entries {
        if out.entries.len() >= MAX_ENTRIES {
            out.truncated = true;
            break;
        }
        let e = entry.map_err(|e| format!("damaged archive: {e}"))?;
        let path = e
            .path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "(unreadable name)".into());
        let dir = e.header().entry_type().is_dir();
        let size = e.header().size().unwrap_or(0);
        push(out, path, size, dir);
    }
    if out.entries.is_empty() && !out.truncated {
        return Err("the archive is empty or damaged".into());
    }
    Ok(())
}

/// What a file that is not text is, from its first bytes and its name.
pub fn describe_binary(path: &Path) -> String {
    let mut head = [0u8; 16];
    let n = std::fs::File::open(path)
        .and_then(|mut f| f.read(&mut head))
        .unwrap_or(0);
    let h = &head[..n];
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let by_magic = if h.starts_with(b"MZ") {
        Some("Windows executable or library")
    } else if h.starts_with(b"\x7fELF") {
        Some("ELF executable or library")
    } else if h.starts_with(&[0xCF, 0xFA, 0xED, 0xFE]) || h.starts_with(&[0xCA, 0xFE, 0xBA, 0xBE]) {
        Some("Mach-O executable or library")
    } else if h.starts_with(b"%PDF") {
        Some("PDF document")
    } else if h.starts_with(b"SQLite format 3") {
        Some("SQLite database")
    } else if h.starts_with(b"\0asm") {
        Some("WebAssembly module")
    } else if h.starts_with(&[0x1F, 0x8B]) {
        Some("gzip-compressed data")
    } else if h.starts_with(b"PK\x03\x04") {
        Some("ZIP-based file")
    } else if h.starts_with(b"7z\xBC\xAF") {
        Some("7-Zip archive")
    } else if h.starts_with(b"Rar!") {
        Some("RAR archive")
    } else if h.starts_with(b"OTTO") || h.starts_with(&[0, 1, 0, 0]) {
        Some("font")
    } else {
        None
    };
    match by_magic {
        Some(kind) => kind.to_string(),
        None if !ext.is_empty() => format!("binary .{ext} file"),
        None => "binary file".into(),
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024. && u < UNITS.len() - 1 {
        v /= 1024.;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tar(dir: &Path) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        b.append_dir_all("app", dir).unwrap();
        b.into_inner().unwrap()
    }

    #[test]
    fn lists_tar_tgz_zip_and_refuses_damaged() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(src.join("lib")).unwrap();
        std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(src.join("lib/данные.txt"), vec![b'x'; 2000]).unwrap();

        let tar_bytes = make_tar(&src);
        let tar_path = d.path().join("Kayo-1.0.1.tar");
        std::fs::write(&tar_path, &tar_bytes).unwrap();
        let tgz_path = d.path().join("Kayo-1.0.1-update.tar.gz");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, &tar_bytes).unwrap();
        std::fs::write(&tgz_path, gz.finish().unwrap()).unwrap();
        let zip_path = d.path().join("a.ZIP");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("readme.md", o).unwrap();
            std::io::Write::write_all(&mut z, b"# hi").unwrap();
            z.add_directory("docs/", o).unwrap();
            z.finish().unwrap();
        }
        for (p, kind) in [
            (&tar_path, ArchiveKind::Tar),
            (&tgz_path, ArchiveKind::TarGz),
        ] {
            assert_eq!(archive_kind(p), Some(kind));
            let l = list(p, kind).unwrap();
            let names: Vec<_> = l.entries.iter().map(|e| e.path.as_str()).collect();
            assert!(names.contains(&"app/main.rs"), "{names:?}");
            assert!(names.contains(&"app/lib/данные.txt"));
            assert_eq!(l.unpacked, 12 + 2000);
        }
        assert_eq!(archive_kind(Path::new("x.tgz")), Some(ArchiveKind::TarGz));
        let l = list(&zip_path, ArchiveKind::Zip).unwrap();
        assert_eq!(l.entries.len(), 2);
        assert!(l.entries.iter().any(|e| e.dir && e.path == "docs/"));

        // Damaged: a cut gzip stream and random bytes.
        let bad = d.path().join("broken.tar.gz");
        let good = std::fs::read(&tgz_path).unwrap();
        std::fs::write(&bad, &good[..good.len() / 3]).unwrap();
        assert!(list(&bad, ArchiveKind::TarGz).is_err());
        let junk = d.path().join("junk.zip");
        std::fs::write(&junk, b"not a zip at all").unwrap();
        assert!(list(&junk, ArchiveKind::Zip).is_err());
        // Nothing was extracted next to the archives.
        assert!(!d.path().join("app").exists());
    }

    #[test]
    fn binary_files_are_described() {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("tool.exe");
        std::fs::write(&exe, b"MZ\x90\0rest").unwrap();
        assert_eq!(describe_binary(&exe), "Windows executable or library");
        let blob = d.path().join("data.bin");
        std::fs::write(&blob, [0u8, 1, 2, 3, 0xFF]).unwrap();
        assert_eq!(describe_binary(&blob), "binary .bin file");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(12), "12 bytes");
    }
}
