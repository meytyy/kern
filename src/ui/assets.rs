//! Embedded assets: KERN icons first, then the component library's defaults.
use gpui_kit::component::{Icon, IconNamed};
use gpui_kit::*;
use std::borrow::Cow;

pub const LOGO_PNG: &[u8] = include_bytes!("../../assets/logo-128.png");

macro_rules! embedded {
    ($($name:literal),* $(,)?) => {
        fn embedded(path: &str) -> Option<&'static [u8]> {
            match path {
                $(concat!("icons/", $name, ".svg") => Some(include_bytes!(concat!("../../assets/icons/", $name, ".svg"))),)*
                _ => None,
            }
        }
    };
}

embedded![
    "agent-codex",
    "history",
    "agent-claude",
    "blocks",
    "package",
    "list-tree",
    "copy-check",
    "image",
    "video",
    "code-xml",
    "zoom-in",
    "zoom-out",
    "maximize-2",
    "volume-2",
    "volume-x",
    "pause",
    "play",
    "git-fork",
    "circle-plus",
    "message-square",
    "clock",
    "git-branch",
    "paperclip",
    "send-horizontal",
    "folder-plus",
    "file-plus",
    "git-pull-request",
    "git-compare",
    "arrow-down-to-line",
    "arrow-up-from-line",
    "circle-stop",
    "terminal",
    "pencil",
    "sparkles",
    "eraser"
];

pub struct KernAssets;

impl AssetSource for KernAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = embedded(path).or_else(|| super::file_icon_assets::embedded(path)) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::AllAssets.list(path)
    }
}

/// An icon by SVG name.
#[derive(Clone, Copy)]
pub struct Ic(pub &'static str);

impl IconNamed for Ic {
    fn path(self) -> SharedString {
        format!("icons/{}.svg", self.0).into()
    }
}

pub fn icon(name: &'static str) -> Icon {
    Icon::new(Ic(name))
}
