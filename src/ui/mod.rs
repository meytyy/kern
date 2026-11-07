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
pub mod keys;
pub mod lsp_bridge;
pub mod media;
pub mod overlays;
pub mod scm;
pub mod search_panel;
pub mod settings_panel;
pub mod start;
pub mod terminal_view;
pub mod theme;
pub mod tree_ops;
pub mod updates;
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
        NewFolder,
        KillTerminal,
        CheckForUpdates,
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
        TreeActivate,
        CancelTreeEdit
    ]
);

fn keymap(cx: &mut App) {
    use keys as k;
    let b = |keys: k::Keys| keys.binding();
    cx.bind_keys([
        KeyBinding::new(b(k::QUICK_OPEN), QuickOpen, None),
        KeyBinding::new(b(k::COMMAND_PALETTE), CommandPalette, None),
        KeyBinding::new(b(k::FIND_IN_PROJECT), FindInProject, None),
        KeyBinding::new(b(k::TOGGLE_SIDEBAR), ToggleSidebar, None),
        KeyBinding::new(b(k::TOGGLE_BOTTOM), ToggleBottom, None),
        KeyBinding::new(b(k::TOGGLE_TERMINAL), ToggleTerminal, None),
        KeyBinding::new(b(k::TOGGLE_CODEX), ToggleCodex, None),
        KeyBinding::new(b(k::OPEN_FOLDER), OpenFolder, None),
        KeyBinding::new(b(k::QUIT), Quit, None),
        KeyBinding::new(b(k::RUN), RunProject, None),
        KeyBinding::new(b(k::STOP), StopProject, None),
        KeyBinding::new(b(k::SETTINGS), OpenSettings, None),
        KeyBinding::new(b(k::EXTENSIONS), ShowExtensions, None),
        KeyBinding::new(b(k::WORD_WRAP), ToggleWordWrap, Some("KernEditor")),
        // Editor only: while a terminal has focus these do not exist.
        KeyBinding::new(b(k::SAVE), Save, Some("KernEditor")),
        KeyBinding::new(b(k::CLOSE_TAB), CloseTab, Some("KernEditor")),
        KeyBinding::new(b(k::NEW_FILE), NewFile, Some("KernEditor")),
        KeyBinding::new(
            b(k::GO_TO_DEFINITION),
            GoToDefinitionHere,
            Some("KernEditor"),
        ),
        KeyBinding::new("escape", Dismiss, Some("KernOverlay")),
        KeyBinding::new("up", TreeUp, Some("KernTree")),
        KeyBinding::new("down", TreeDown, Some("KernTree")),
        KeyBinding::new("left", TreeLeft, Some("KernTree")),
        KeyBinding::new("right", TreeRight, Some("KernTree")),
        KeyBinding::new("enter", TreeActivate, Some("KernTree")),
        KeyBinding::new("space", TreeActivate, Some("KernTree")),
        // The name field in the tree: Esc cancels, and the tree's own keys
        // (Space opens, arrows move) do not apply while typing a name.
        KeyBinding::new("escape", CancelTreeEdit, Some("KernTreeEdit")),
        KeyBinding::new("space", NoAction, Some("KernTreeEdit")),
        KeyBinding::new("up", NoAction, Some("KernTreeEdit")),
        KeyBinding::new("down", NoAction, Some("KernTreeEdit")),
        KeyBinding::new("left", NoAction, Some("KernTreeEdit")),
        KeyBinding::new("right", NoAction, Some("KernTreeEdit")),
        KeyBinding::new("enter", NoAction, Some("KernTreeEdit")),
    ]);
    if !cfg!(target_os = "macos") {
        // A shell owns Ctrl+letter (Ctrl+R history, Ctrl+B back...): with a terminal
        // focused these keys reach it, not the application.
        let shell_keys = [
            k::QUICK_OPEN,
            k::TOGGLE_SIDEBAR,
            k::TOGGLE_BOTTOM,
            k::OPEN_FOLDER,
            k::QUIT,
            k::RUN,
            k::STOP,
            k::SETTINGS,
        ];
        cx.bind_keys(
            shell_keys.map(|keys| KeyBinding::new(keys.binding(), NoAction, Some("Terminal"))),
        );
    }
    terminal_view::bind_keys(cx);
}

fn menu_list() -> Vec<Menu> {
    use gpui_kit::component::input as gi;
    let menu = |name: &str, items| Menu {
        name: name.to_string().into(),
        disabled: false,
        items,
    };
    vec![
        menu(
            "KERN",
            vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::action("Quit KERN", Quit),
            ],
        ),
        menu(
            "File",
            vec![
                MenuItem::action("Open Folder…", OpenFolder),
                MenuItem::action("New File…", NewFile),
                MenuItem::action("New Folder…", NewFolder),
                MenuItem::separator(),
                MenuItem::action("Save", Save),
                MenuItem::action("Close Tab", CloseTab),
            ],
        ),
        menu(
            "Edit",
            vec![
                MenuItem::action("Undo", gi::Undo),
                MenuItem::action("Redo", gi::Redo),
                MenuItem::separator(),
                MenuItem::action("Cut", gi::Cut),
                MenuItem::action("Copy", gi::Copy),
                MenuItem::action("Paste", gi::Paste),
                MenuItem::action("Select All", gi::SelectAll),
            ],
        ),
        menu(
            "View",
            vec![
                MenuItem::action("Command Palette", CommandPalette),
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
                MenuItem::action("Toggle Bottom Panel", ToggleBottom),
                MenuItem::action("Toggle Codex Panel", ToggleCodex),
                MenuItem::action("Toggle Theme", ToggleTheme),
            ],
        ),
        menu(
            "Run",
            vec![
                MenuItem::action("Run", RunProject),
                MenuItem::action("Stop", StopProject),
            ],
        ),
        // The same actions as the panel buttons, Ctrl+` and the command palette.
        menu(
            "Terminal",
            vec![
                MenuItem::action("New Terminal", NewTerminal),
                MenuItem::action("Show / Hide Terminal", ToggleTerminal),
                MenuItem::action("Kill Terminal", KillTerminal),
                MenuItem::separator(),
                MenuItem::action("Open Claude Code", OpenClaude),
            ],
        ),
        menu(
            "Help",
            vec![MenuItem::action("Check for Updates…", CheckForUpdates)],
        ),
    ]
}

/// macOS shows these in the system menu bar. Windows has none for this window
/// style: the title bar draws them (`AppMenuBar`) from the same list.
fn menus(cx: &mut App) {
    cx.set_menus(menu_list());
    let owned = menu_list().into_iter().map(Menu::owned).collect();
    gpui_kit::base::GlobalState::global_mut(cx).set_app_menus(owned);
}

/// Whether the window draws its own menu bar. `KERN_IN_WINDOW_MENU=1` shows the
/// Windows one on macOS too, for testing.
pub fn in_window_menu() -> bool {
    !cfg!(target_os = "macos") || std::env::var_os("KERN_IN_WINDOW_MENU").is_some()
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
