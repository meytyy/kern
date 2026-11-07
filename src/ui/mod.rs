//! The native user interface (GPUI). Each panel lives in its own file; all of
//! them are `impl KernApp` blocks over one application state.
pub mod agent_chat;
pub mod agents_panel;
pub mod app;
pub mod assets;
pub mod bottom;
pub mod codex_panel;
pub mod editor_area;
pub mod extensions;
pub mod file_icon_assets;
pub mod file_icons;
pub mod files;
pub mod grammars;
pub mod graph_panel;
pub mod lsp_bridge;
pub mod media;
pub mod overlays;
pub mod scm;
pub mod search_panel;
pub mod settings_panel;
pub mod start;
pub mod terminal_view;
pub mod theme;
pub mod widgets;

#[cfg(feature = "autotest")]
pub mod autotest;

use crate::core::settings::Settings;
use app::KernApp;
use gpui_kit::*;

gpui_kit::actions!(
    kern,
    [
        Save,
        QuickOpen,
        CommandPalette,
        FindInProject,
        ToggleSidebar,
        ToggleBottom,
        ToggleCodex,
        NewTerminal,
        CloseTab,
        OpenFolder,
        RunProject,
        StopProject,
        ToggleTheme,
        OpenClaude,
        Quit,
        Dismiss,
        NewFile,
        GoToDefinitionHere,
        OpenSettings,
        ToggleWordWrap,
        ChangeLanguage,
        ShowExtensions,
        ToggleTerminal,
        TreeUp,
        TreeDown,
        TreeLeft,
        TreeRight,
        TreeActivate
    ]
);

fn keymap(cx: &mut App) {
    // ⌘ on macOS, Ctrl elsewhere. Ctrl+Alt is AltGr on Windows, so no binding uses it there.
    let m = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    // Built once at startup; the keymap keeps the strings for the app's lifetime.
    let k = |keys: &str| -> &'static str { Box::leak(keys.replace("cmd", m).into_boxed_str()) };
    let codex = if cfg!(target_os = "macos") {
        "cmd-alt-c"
    } else {
        "ctrl-shift-a"
    };
    cx.bind_keys([
        KeyBinding::new(k("cmd-p"), QuickOpen, None),
        KeyBinding::new(k("cmd-shift-p"), CommandPalette, None),
        KeyBinding::new(k("cmd-shift-f"), FindInProject, None),
        KeyBinding::new(k("cmd-b"), ToggleSidebar, None),
        KeyBinding::new(k("cmd-j"), ToggleBottom, None),
        KeyBinding::new("ctrl-`", ToggleTerminal, None),
        KeyBinding::new(codex, ToggleCodex, None),
        KeyBinding::new(k("cmd-o"), OpenFolder, None),
        KeyBinding::new(k("cmd-q"), Quit, None),
        KeyBinding::new(k("cmd-r"), RunProject, None),
        KeyBinding::new(k("cmd-."), StopProject, None),
        KeyBinding::new(k("cmd-,"), OpenSettings, None),
        KeyBinding::new(k("cmd-shift-x"), ShowExtensions, None),
        KeyBinding::new("alt-z", ToggleWordWrap, Some("KernEditor")),
        // Editor only: while a terminal has focus these do not exist.
        KeyBinding::new(k("cmd-s"), Save, Some("KernEditor")),
        KeyBinding::new(k("cmd-w"), CloseTab, Some("KernEditor")),
        KeyBinding::new(k("cmd-n"), NewFile, Some("KernEditor")),
        KeyBinding::new("f12", GoToDefinitionHere, Some("KernEditor")),
        KeyBinding::new("escape", Dismiss, Some("KernOverlay")),
        KeyBinding::new("up", TreeUp, Some("KernTree")),
        KeyBinding::new("down", TreeDown, Some("KernTree")),
        KeyBinding::new("left", TreeLeft, Some("KernTree")),
        KeyBinding::new("right", TreeRight, Some("KernTree")),
        KeyBinding::new("enter", TreeActivate, Some("KernTree")),
        KeyBinding::new("space", TreeActivate, Some("KernTree")),
    ]);
    if !cfg!(target_os = "macos") {
        // A shell owns Ctrl+letter (Ctrl+R history, Ctrl+B back...): with a terminal
        // focused these keys reach it, not the application.
        cx.bind_keys(
            [
                "ctrl-p", "ctrl-b", "ctrl-j", "ctrl-o", "ctrl-q", "ctrl-r", "ctrl-.", "ctrl-,",
            ]
            .map(|key| KeyBinding::new(key, NoAction, Some("Terminal"))),
        );
    }
    terminal_view::bind_keys(cx);
}

fn menus(cx: &mut App) {
    use gpui_kit::component::input as gi;
    cx.set_menus(vec![
        Menu {
            name: "KERN".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::action("Quit KERN", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Open Folder…", OpenFolder),
                MenuItem::action("New File", NewFile),
                MenuItem::action("Save", Save),
                MenuItem::action("Close Tab", CloseTab),
            ],
        },
        Menu {
            name: "Edit".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Undo", gi::Undo),
                MenuItem::action("Redo", gi::Redo),
                MenuItem::separator(),
                MenuItem::action("Cut", gi::Cut),
                MenuItem::action("Copy", gi::Copy),
                MenuItem::action("Paste", gi::Paste),
                MenuItem::action("Select All", gi::SelectAll),
            ],
        },
        Menu {
            name: "View".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Command Palette", CommandPalette),
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
                MenuItem::action("Toggle Bottom Panel", ToggleBottom),
                MenuItem::action("Toggle Codex Panel", ToggleCodex),
                MenuItem::action("Toggle Theme", ToggleTheme),
            ],
        },
        Menu {
            name: "Run".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Run", RunProject),
                MenuItem::action("Stop", StopProject),
                MenuItem::action("Open Claude Code", OpenClaude),
                MenuItem::action("Toggle Terminal", ToggleTerminal),
                MenuItem::action("New Terminal", NewTerminal),
            ],
        },
    ]);
}

/// Application entry point.
pub fn run() {
    let settings = Settings::load();
    crate::platform::tools::set_overrides(settings.tool_paths.clone());
    let shell = settings.shell.clone();
    std::thread::Builder::new()
        .name("login-env".into())
        .spawn(move || crate::platform::tools::initialize_environment(shell.as_deref()))
        .ok();

    let initial_project = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir());

    gpui_kit::application()
        .with_assets(assets::KernAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            grammars::register();
            theme::apply(&settings, cx);
            keymap(cx);
            menus(cx);
            cx.on_action(|_: &Quit, cx: &mut App| {
                // Reached only when no window handled it (no window open).
                cx.quit();
            });
            // One window per app for now: closing it ends the app and its processes.
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let layout = settings.layout.clone();
            let bounds = Bounds::centered(
                None,
                size(px(layout.window_width), px(layout.window_height)),
                cx,
            );
            let opts = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(720.), px(480.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("KERN".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.), px(12.))),
                }),
                app_id: Some("dev.kern.ide".into()),
                ..Default::default()
            };
            let settings2 = settings.clone();
            let opened = gpui_kit::open_window(opts, cx, move |window, cx| {
                cx.new(|cx| KernApp::new(settings2, initial_project, window, cx))
            });
            if let Err(e) = opened {
                eprintln!("KERN: cannot open window: {e}");
                cx.quit();
            }
            #[cfg(not(feature = "autotest"))]
            cx.activate(true);
        });
}
