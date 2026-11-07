//! Signed update packages. Every release file has a `<file>.sig` next to it:
//! the version, file name, size and SHA-256 of the package, signed with
//! Ed25519 (the scheme Sparkle and Tauri use for app updates). KERN carries
//! the public key; the private key never leaves the release machine. A package
//! is installed only if its signature verifies and its bytes match.
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

/// The release key. Test builds may embed a throwaway key at compile time with
/// `KERN_UPDATE_PUBKEY`; release builds never set it.
pub const PUBLIC_KEY_HEX: &str = match option_env!("KERN_UPDATE_PUBKEY") {
    Some(k) => k,
    None => "0d21615d6bf3ef47313116883430816dbccccf8e5a7926de34c88925d833cfe4",
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
}

impl Manifest {
    fn message(&self) -> String {
        format!(
            "kern-update-v1\nversion: {}\nfile: {}\nsize: {}\nsha256: {}\n",
            self.version, self.file, self.size, self.sha256
        )
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let text = text.trim();
    if text.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, chunk) in text.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(out)
}

pub fn sha256_file(path: &Path) -> std::io::Result<(u64, String)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        hasher.update(&buf[..n]);
    }
    Ok((size, hex(&hasher.finalize())))
}

/// The `.sig` file for a package (release tooling).
pub fn sign(secret: &[u8; 32], version: &str, package: &Path) -> std::io::Result<String> {
    let (size, sha256) = sha256_file(package)?;
    let file = package
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let m = Manifest {
        version: version.to_string(),
        file,
        size,
        sha256,
    };
    let key = SigningKey::from_bytes(secret);
    let sig = key.sign(m.message().as_bytes());
    Ok(format!(
        "{}signature: {}\n",
        m.message(),
        base64::engine::general_purpose::STANDARD.encode(sig.to_bytes())
    ))
}

pub fn public_key_of(secret: &[u8; 32]) -> String {
    hex(SigningKey::from_bytes(secret).verifying_key().as_bytes())
}

/// Reads a `.sig` file and checks its signature with `public_key_hex`.
pub fn verify(text: &str, public_key_hex: &str) -> Result<Manifest, String> {
    let field = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{name}: ")))
            .map(str::to_string)
            .ok_or_else(|| format!("the signature file has no {name}"))
    };
    if !text.starts_with("kern-update-v1\n") {
        return Err("not a KERN update signature".into());
    }
    let m = Manifest {
        version: field("version")?,
        file: field("file")?,
        size: field("size")?
            .parse()
            .map_err(|_| "bad size in the signature file")?,
        sha256: field("sha256")?,
    };
    let sig = base64::engine::general_purpose::STANDARD
        .decode(field("signature")?.trim())
        .map_err(|_| "bad signature encoding")?;
    let sig: [u8; 64] = sig.try_into().map_err(|_| "bad signature length")?;
    let key = unhex::<32>(public_key_hex).ok_or("bad public key")?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| "bad public key")?;
    key.verify_strict(m.message().as_bytes(), &Signature::from_bytes(&sig))
        .map_err(|_| "the update is not signed by KERN's release key".to_string())?;
    Ok(m)
}

/// The downloaded bytes are the signed ones (and, when GitHub reports it,
/// GitHub's own SHA-256 of the asset agrees).
pub fn check_package(path: &Path, m: &Manifest, github_sha256: Option<&str>) -> Result<(), String> {
    let (size, sha) = sha256_file(path).map_err(|e| format!("cannot read the download: {e}"))?;
    if size != m.size {
        return Err(format!(
            "the download is incomplete ({size} of {} bytes)",
            m.size
        ));
    }
    if sha != m.sha256 {
        return Err("the download is damaged: its SHA-256 does not match the signature".into());
    }
    if let Some(g) = github_sha256
        && g != sha
    {
        return Err("the download does not match GitHub's checksum".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_package_verifies_and_tampering_is_caught() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("KERN-windows-x64-setup.exe");
        std::fs::write(&pkg, b"installer bytes").unwrap();
        let secret = [7u8; 32];
        let public = public_key_of(&secret);
        let sig = sign(&secret, "0.1.4", &pkg).unwrap();
        let m = verify(&sig, &public).unwrap();
        assert_eq!(
            (m.version.as_str(), m.file.as_str(), m.size),
            ("0.1.4", "KERN-windows-x64-setup.exe", 15)
        );
        check_package(&pkg, &m, None).unwrap();
        check_package(&pkg, &m, Some(&m.sha256)).unwrap();
        assert!(check_package(&pkg, &m, Some("00")).is_err());
        // Another key, or an edited manifest, fails.
        assert!(verify(&sig, &public_key_of(&[8u8; 32])).is_err());
        assert!(verify(&sig.replace("0.1.4", "0.1.5"), &public).is_err());
        // A damaged or truncated package fails.
        std::fs::write(&pkg, b"installer bytez").unwrap();
        assert!(
            check_package(&pkg, &m, None)
                .unwrap_err()
                .contains("damaged")
        );
        std::fs::write(&pkg, b"installer").unwrap();
        assert!(
            check_package(&pkg, &m, None)
                .unwrap_err()
                .contains("incomplete")
        );
    }
}
