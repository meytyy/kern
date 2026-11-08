//! Generic Language Server Protocol client.
pub mod client;
pub mod position;
pub mod transport;

pub use client::{LspClient, LspEvent, ServerStatus};
pub use position::Encoding;

/// file:// URI for a path. Handles spaces and non-ASCII names.
pub fn path_to_uri(path: &std::path::Path) -> String {
    url::Url::from_file_path(path)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| format!("file://{}", path.display()))
}

pub fn uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    url::Url::parse(uri).ok()?.to_file_path().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn uri_roundtrip_with_spaces_and_cyrillic() {
        let (p, prefix) = if cfg!(windows) {
            (Path::new(r"C:\мой проект\App file.tsx"), "file:///C:/")
        } else {
            (Path::new("/tmp/мой проект/App file.tsx"), "file:///tmp/")
        };
        let uri = path_to_uri(p);
        assert!(uri.starts_with(prefix) && !uri.contains(' '), "{uri}");
        assert_eq!(uri_to_path(&uri).unwrap(), p);
    }
}
