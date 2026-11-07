//! Scripted driver for verifying flows in a real window (feature `autotest`).
//! Steps use the same dispatch path as real key presses, and real providers.
use super::app::*;
use super::editor_area::Tab;
use gpui_kit::component::input::Rope;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::{Duration, Instant};

fn at_agent(_name: &str) -> super::agents_panel::AgentTab {
    super::agents_panel::AgentTab::Codex
}

/// Offers one fixed item, so key handling can be tested without a language server.
struct FakeCompletions(String);

impl gpui_kit::component::input::CompletionProvider for FakeCompletions {
    fn completions(
        &self,
        _text: &Rope,
        _offset: usize,
        _trigger: lsp_types::CompletionContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<anyhow::Result<lsp_types::CompletionResponse>> {
        let item = lsp_types::CompletionItem {
            label: self.0.clone(),
            ..Default::default()
        };
        Task::ready(Ok(lsp_types::CompletionResponse::Array(vec![item])))
    }

    fn is_completion_trigger(&self, _offset: usize, _new_text: &str, _cx: &mut App) -> bool {
        false
    }
}

fn log(s: &str) {
    println!("[autotest] {s}");
    // KERN_AUTOTEST_LOG: also a file, for runs whose output nobody captures
    // (a KERN started again by the update installer).
    if let Some(path) = std::env::var_os("KERN_AUTOTEST_LOG") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "[autotest {}] {s}", env!("CARGO_PKG_VERSION"));
        }
    }
}

pub fn maybe_start(window: &mut Window, cx: &mut Context<KernApp>) {
    let Ok(path) = std::env::var("KERN_AUTOTEST") else {
        return;
    };
    // Scripts change settings. They must never run on someone's real configuration.
    assert!(
        std::env::var_os("KERN_CONFIG_DIR").is_some(),
        "KERN_AUTOTEST needs KERN_CONFIG_DIR"
    );
    #[cfg(target_os = "macos")]
    if std::env::var_os("KERN_AUTOTEST_FLOAT").is_some() {
        float_test_window();
    }
    let script = std::fs::read_to_string(&path).expect("script");
    let lines: Vec<String> = script
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();
    cx.spawn_in(window, async move |this, cx| {
        let mut failures = 0;
        cx.background_executor()
            .timer(Duration::from_millis(800))
            .await;
        for line in lines {
            // `onversion 0.1.4 <step>`: the step runs only in that version, so one
            // script can drive KERN before and after an in-app update.
            let line = match line.strip_prefix("onversion ") {
                Some(rest) => {
                    let (v, step) = rest.split_once(' ').unwrap_or((rest, ""));
                    if v != env!("CARGO_PKG_VERSION") {
                        continue;
                    }
                    step.to_string()
                }
                None => line,
            };
            let (cmd, arg) = line.split_once(' ').unwrap_or((line.as_str(), ""));
            let (cmd, arg) = (cmd.to_string(), arg.to_string());
            let started = Instant::now();
            let result: Result<String, String> = match cmd.as_str() {
                "sleep" => {
                    cx.background_executor()
                        .timer(Duration::from_millis(arg.parse().unwrap_or(100)))
                        .await;
                    Ok(String::new())
                }
                "waitfor" => {
                    let (secs, cond) = arg.split_once(' ').unwrap_or(("10", &arg));
                    let deadline = Instant::now() + Duration::from_secs(secs.parse().unwrap_or(10));
                    let cond = cond.to_string();
                    loop {
                        let r = this
                            .update_in(cx, |this, w, cx| this.at_check(&cond, w, cx))
                            .unwrap_or_else(|e| Err(e.to_string()));
                        match r {
                            Ok(s) => break Ok(s),
                            Err(e) if Instant::now() > deadline => {
                                break Err(format!("timeout: {e}"));
                            }
                            Err(_) => {
                                cx.background_executor()
                                    .timer(Duration::from_millis(150))
                                    .await
                            }
                        }
                    }
                }
                "complete" => {
                    let task = this.update_in(cx, |this, w, cx| this.at_completion(w, cx));
                    match task {
                        Ok(Some(t)) => match t.await {
                            Ok(labels) if labels.iter().any(|l| l.contains(&arg)) => {
                                Ok(format!("{} items include {arg}", labels.len()))
                            }
                            Ok(labels) => Err(format!(
                                "{arg} not in {} items: {:?}",
                                labels.len(),
                                labels.iter().take(12).collect::<Vec<_>>()
                            )),
                            Err(e) => Err(e.to_string()),
                        },
                        _ => Err("no completion provider".into()),
                    }
                }
                "hover" => {
                    let task = this.update_in(cx, |this, w, cx| this.at_hover(w, cx));
                    match task {
                        Ok(Some(t)) => match t.await {
                            Ok(Some(text)) if text.contains(&arg) => {
                                Ok(text.chars().take(100).collect())
                            }
                            Ok(other) => Err(format!("hover was {other:?}")),
                            Err(e) => Err(e.to_string()),
                        },
                        _ => Err("no hover provider".into()),
                    }
                }
                "shot" => {
                    let name = arg.clone();
                    cx.background_spawn(async move {
                        let cmd = std::env::var("KERN_SHOT_CMD").unwrap_or_default();
                        if !cmd.is_empty() {
                            let _ = std::process::Command::new(cmd)
                                .arg(std::process::id().to_string())
                                .arg(&name)
                                .status();
                        }
                    })
                    .await;
                    Ok(String::new())
                }
                "key" => {
                    let leaked: &'static str = Box::leak(arg.clone().into_boxed_str());
                    match Keystroke::parse(leaked) {
                        Ok(k) => cx
                            .update(|w, cx| {
                                // Enter and Space click a focused control on release.
                                w.dispatch_keystroke(k.clone(), cx);
                                w.dispatch_event(
                                    PlatformInput::KeyUp(KeyUpEvent { keystroke: k }),
                                    cx,
                                );
                                String::new()
                            })
                            .map_err(|e| e.to_string()),
                        Err(e) => Err(e.to_string()),
                    }
                }
                "type" => {
                    let text = arg.clone();
                    cx.update(|w, cx| {
                        for ch in text.chars() {
                            let s = ch.to_string();
                            let k = Keystroke {
                                modifiers: Modifiers::default(),
                                key: if ch == ' ' { "space".into() } else { s.clone() },
                                key_char: Some(s),
                            };
                            w.dispatch_keystroke(k, cx);
                        }
                        String::new()
                    })
                    .map_err(|e| e.to_string())
                }
                "drop" => {
                    // A real file drop through the window, as Finder delivers it.
                    let (xy, file) = arg.split_once('|').unwrap_or((arg.as_str(), ""));
                    let parts: Vec<f32> = xy.split(' ').filter_map(|v| v.parse().ok()).collect();
                    let position = point(px(parts[0]), px(parts[1]));
                    let paths = ExternalPaths(smallvec::smallvec![std::path::PathBuf::from(file)]);
                    cx.update(|w, cx| {
                        w.dispatch_event(
                            PlatformInput::FileDrop(FileDropEvent::Entered { position, paths }),
                            cx,
                        );
                        w.dispatch_event(
                            PlatformInput::FileDrop(FileDropEvent::Submit { position }),
                            cx,
                        );
                        String::new()
                    })
                    .map_err(|e| e.to_string())
                }
                "dragmove" => {
                    let parts: Vec<f32> = arg.split(' ').filter_map(|v| v.parse().ok()).collect();
                    let position = point(px(parts[0]), px(parts[1]));
                    cx.update(|w, cx| {
                        w.dispatch_event(
                            PlatformInput::MouseMove(MouseMoveEvent {
                                position,
                                pressed_button: Some(MouseButton::Left),
                                modifiers: Modifiers::default(),
                            }),
                            cx,
                        );
                        String::new()
                    })
                    .map_err(|e| e.to_string())
                }
                "dragdown" | "dragup" => {
                    let parts: Vec<f32> = arg.split(' ').filter_map(|v| v.parse().ok()).collect();
                    let position = point(px(parts[0]), px(parts[1]));
                    let down = cmd == "dragdown";
                    cx.update(|w, cx| {
                        let input = if down {
                            PlatformInput::MouseDown(MouseDownEvent {
                                button: MouseButton::Left,
                                position,
                                modifiers: Modifiers::default(),
                                click_count: 1,
                                first_mouse: false,
                            })
                        } else {
                            PlatformInput::MouseUp(MouseUpEvent {
                                button: MouseButton::Left,
                                position,
                                modifiers: Modifiers::default(),
                                click_count: 1,
                            })
                        };
                        w.dispatch_event(input, cx);
                        String::new()
                    })
                    .map_err(|e| e.to_string())
                }
                "rpress" => {
                    // Right click, for context menus.
                    let parts: Vec<f32> = arg.split(' ').filter_map(|v| v.parse().ok()).collect();
                    let position = point(px(parts[0]), px(parts[1]));
                    cx.update(|w, cx| {
                        for down in [true, false] {
                            let input = if down {
                                PlatformInput::MouseDown(MouseDownEvent {
                                    button: MouseButton::Right,
                                    position,
                                    modifiers: Modifiers::default(),
                                    click_count: 1,
                                    first_mouse: false,
                                })
                            } else {
                                PlatformInput::MouseUp(MouseUpEvent {
                                    button: MouseButton::Right,
                                    position,
                                    modifiers: Modifiers::default(),
                                    click_count: 1,
                                })
                            };
                            w.dispatch_event(input, cx);
                        }
                        String::new()
                    })
                    .map_err(|e| e.to_string())
                }
                "action" => {
                    // The path a menu item takes: an action dispatched to the focus.
                    use super::*;
                    let action: Option<Box<dyn Action>> = match arg.as_str() {
                        "NewTerminal" => Some(Box::new(NewTerminal)),
                        "ToggleTerminal" => Some(Box::new(ToggleTerminal)),
                        "KillTerminal" => Some(Box::new(KillTerminal)),
                        "OpenClaude" => Some(Box::new(OpenClaude)),
                        "NewFile" => Some(Box::new(NewFile)),
                        "NewFolder" => Some(Box::new(NewFolder)),
                        "CheckForUpdates" => Some(Box::new(CheckForUpdates)),
                        "ToggleBottom" => Some(Box::new(ToggleBottom)),
                        "ToggleSidebar" => Some(Box::new(ToggleSidebar)),
                        "DownloadUpdate" => Some(Box::new(DownloadUpdate)),
                        "RestartToUpdate" => Some(Box::new(RestartToUpdate)),
                        "AboutKern" => Some(Box::new(AboutKern)),
                        _ => None,
                    };
                    match action {
                        Some(a) => cx
                            .update(|w, cx| {
                                w.dispatch_action(a, cx);
                                String::new()
                            })
                            .map_err(|e| e.to_string()),
                        None => Err(format!("unknown action {arg}")),
                    }
                }
                "mouse" | "press" => {
                    // Real input through the window's hit testing, in logical pixels.
                    let parts: Vec<f32> = arg.split(' ').filter_map(|v| v.parse().ok()).collect();
                    let position = point(px(parts[0]), px(parts[1]));
                    let is_move = cmd == "mouse";
                    cx.update(|w, cx| {
                        if is_move {
                            w.dispatch_event(
                                PlatformInput::MouseMove(MouseMoveEvent {
                                    position,
                                    pressed_button: None,
                                    modifiers: Modifiers::default(),
                                }),
                                cx,
                            );
                        } else {
                            w.dispatch_event(
                                PlatformInput::MouseDown(MouseDownEvent {
                                    button: MouseButton::Left,
                                    position,
                                    modifiers: Modifiers::default(),
                                    click_count: 1,
                                    first_mouse: false,
                                }),
                                cx,
                            );
                            w.dispatch_event(
                                PlatformInput::MouseUp(MouseUpEvent {
                                    button: MouseButton::Left,
                                    position,
                                    modifiers: Modifiers::default(),
                                    click_count: 1,
                                }),
                                cx,
                            );
                        }
                        String::new()
                    })
                    .map_err(|e| e.to_string())
                }
                "shell" => {
                    let script = arg.clone();
                    cx.background_spawn(async move {
                        // cmd parses its own command line: pass it as written, or
                        // Rust's quoting of embedded quotes garbles the paths.
                        #[cfg(windows)]
                        let mut command = {
                            use std::os::windows::process::CommandExt;
                            let mut c = std::process::Command::new("cmd");
                            c.raw_arg("/C").raw_arg(&script);
                            c
                        };
                        #[cfg(not(windows))]
                        let mut command = {
                            let mut c = std::process::Command::new("/bin/sh");
                            c.arg("-c");
                            c
                        };
                        #[cfg(not(windows))]
                        command.arg(&script);
                        let out = command.output().map_err(|e| e.to_string())?;
                        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        if out.status.success() {
                            Ok(text)
                        } else {
                            Err(format!("exit {:?}: {text}", out.status.code()))
                        }
                    })
                    .await
                }
                "quit" => {
                    log(&format!("done, {failures} failures"));
                    let _ = cx.update(|_, cx| cx.quit());
                    return;
                }
                _ => this
                    .update_in(cx, |this, w, cx| this.at_step(&cmd, &arg, w, cx))
                    .unwrap_or_else(|e| Err(e.to_string())),
            };
            match result {
                Ok(s) => log(&format!(
                    "ok   {line}  ({} ms) {s}",
                    started.elapsed().as_millis()
                )),
                Err(e) => {
                    failures += 1;
                    log(&format!("FAIL {line}: {e}"));
                }
            }
        }
    })
    .detach();
}

/// macOS stops redrawing a window that other windows cover completely, and clicks
/// then hit a stale frame. Keeps the test window above others without activating
/// it: the keyboard stays with whatever the person at the Mac is using.
#[cfg(target_os = "macos")]
fn float_test_window() {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    unsafe {
        let Some(class) = AnyClass::get(c"NSApplication") else {
            return;
        };
        let app: Retained<AnyObject> = msg_send![class, sharedApplication];
        let windows: Retained<AnyObject> = msg_send![&*app, windows];
        let win: Option<Retained<AnyObject>> = msg_send![&*windows, firstObject];
        if let Some(win) = win {
            let _: () = msg_send![&*win, setLevel: 3isize];
            let _: () = msg_send![&*win, orderFrontRegardless];
        }
    }
}

/// Works on the real AppKit window and its standard buttons.
#[cfg(not(target_os = "macos"))]
fn native_window_check(_arg: &str) -> Result<String, String> {
    Err("native window checks are macOS-only".into())
}

#[cfg(target_os = "macos")]
fn native_window_check(arg: &str) -> Result<String, String> {
    use objc2::Encode;
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    unsafe impl Encode for Rect {
        const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
            "CGRect",
            &[
                objc2::Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]),
                objc2::Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]),
            ],
        );
    }
    unsafe {
        let class = AnyClass::get(c"NSApplication").ok_or("no NSApplication")?;
        let app: Retained<AnyObject> = msg_send![class, sharedApplication];
        let windows: Retained<AnyObject> = msg_send![&*app, windows];
        let win: Option<Retained<AnyObject>> = msg_send![&*windows, firstObject];
        let win = win.ok_or("no window")?;
        let button = |kind: usize| -> Option<Retained<AnyObject>> {
            msg_send![&*win, standardWindowButton: kind]
        };
        let click = |kind: usize| -> Result<String, String> {
            let b = button(kind).ok_or("no button")?;
            let _: () = msg_send![&*b, performClick: Option::<&AnyObject>::None];
            Ok(String::new())
        };
        let mask: usize = msg_send![&*win, styleMask];
        match arg {
            "report" => {
                let mut out = String::new();
                for (name, kind) in [("close", 0usize), ("min", 1), ("zoom", 2)] {
                    let b = button(kind).ok_or("no button")?;
                    let frame: Rect = msg_send![&*b, frame];
                    let enabled: bool = msg_send![&*b, isEnabled];
                    let sup: Retained<AnyObject> = msg_send![&*b, superview];
                    let sf: Rect = msg_send![&*sup, frame];
                    let top = sf.h - frame.y - frame.h;
                    out.push_str(&format!(
                        "{name}: x={} top={} {}x{} enabled={enabled}; ",
                        frame.x, top, frame.w, frame.h
                    ));
                }
                Ok(out)
            }
            "close" => click(0),
            "min" => click(1),
            "zoom" => click(2),
            "isminiaturized" => {
                let v: bool = msg_send![&*win, isMiniaturized];
                if v {
                    Ok(String::new())
                } else {
                    Err("not minimized".into())
                }
            }
            "deminiaturize" => {
                let _: () = msg_send![&*win, deminiaturize: Option::<&AnyObject>::None];
                Ok(String::new())
            }
            "isfullscreen" => {
                if mask & (1 << 14) != 0 {
                    Ok(String::new())
                } else {
                    Err(format!("mask {mask:#x}"))
                }
            }
            "notfullscreen" => {
                if mask & (1 << 14) == 0 {
                    Ok(String::new())
                } else {
                    Err(format!("mask {mask:#x}"))
                }
            }
            _ => Err("window what?".into()),
        }
    }
}

impl KernApp {
    fn at_active_editor(&self) -> Result<Entity<gpui_kit::component::input::EditorState>, String> {
        self.active_file()
            .map(|f| f.editor.clone())
            .ok_or_else(|| "no active file".to_string())
    }

    fn at_terminal(&self) -> Result<Entity<super::terminal_view::TerminalView>, String> {
        let p = self.project.as_ref().ok_or("no project")?;
        p.terminals
            .get(p.active_terminal)
            .cloned()
            .ok_or_else(|| "no terminal".to_string())
    }

    fn at_step(
        &mut self,
        cmd: &str,
        arg: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        match cmd {
            "open" => self.open_project(arg.into(), w, cx),
            "trust" => self.trust_project(w, cx),
            "showcompletions" => {
                let e = self.at_active_editor()?;
                e.update(cx, |e, cx| e.show_completions(w, cx));
            }
            "fakecompletions" => {
                // Stands in for a language server: this test is about the keys.
                let e = self.at_active_editor()?;
                e.update(cx, |e, _| {
                    e.lsp_mut().completion_provider =
                        Some(std::rc::Rc::new(FakeCompletions(arg.to_string())));
                });
            }
            "openfile" => {
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                self.open_file_async(root.join(arg), true, None, w, cx);
            }
            "goto" => {
                let (l, c) = arg.split_once(' ').ok_or("line col")?;
                let e = self.at_active_editor()?;
                let pos = lsp_types::Position::new(
                    l.parse().map_err(|_| "line")?,
                    c.parse().map_err(|_| "col")?,
                );
                e.update(cx, |e, cx| e.set_cursor_position(pos, w, cx));
            }
            "focus" => match arg {
                "editor" => {
                    let e = self.at_active_editor()?;
                    e.update(cx, |e, cx| e.focus(w, cx));
                }
                "terminal" => {
                    let t = self.at_terminal()?;
                    t.update(cx, |t, cx| t.focus(w, cx));
                }
                "claude" => {
                    let c = self
                        .project
                        .as_ref()
                        .and_then(|p| p.claude_code.clone())
                        .ok_or("no claude")?;
                    c.update(cx, |t, cx| t.focus(w, cx));
                }
                _ => return Err("focus what?".into()),
            },
            "side" => {
                self.side = match arg {
                    "files" => SideView::Files,
                    "search" => SideView::Search,
                    "scm" => SideView::SourceControl,
                    "extensions" => SideView::Extensions,
                    _ => return Err("side?".into()),
                };
                self.settings.layout.sidebar_visible = true;
            }
            "panel" => {
                let p = self.project.as_mut().ok_or("no project")?;
                p.bottom_tab = match arg {
                    "terminal" => BottomTab::Terminal,
                    "run" => BottomTab::RunOutput,
                    "claude" => BottomTab::Claude,
                    _ => return Err("panel?".into()),
                };
                self.settings.layout.bottom_visible = true;
            }
            "theme" => self.toggle_theme(cx),
            "layout" => {
                let (k, v) = arg.split_once(' ').ok_or("layout k v")?;
                let v: f32 = v.parse().map_err(|_| "v")?;
                match k {
                    "sidebar" => self.settings.layout.sidebar_width = v,
                    "bottom" => self.settings.layout.bottom_height = v,
                    "codex" => self.settings.layout.codex_width = v,
                    _ => return Err("k".into()),
                }
                self.persist();
            }
            "togglecodex" => self.toggle_panel(Dragging::Codex, w, cx),
            "opencodex" => {
                self.settings.layout.codex_visible = true;
                self.codex_connect(cx);
            }
            "runsel" => {
                let p = self.project.as_mut().ok_or("no project")?;
                p.run.selected = Some(arg.to_string());
            }
            "run" => self.run_selected(w, cx),
            "stop" => self.stop_run(cx),
            "toCodex" => self.send_output_to_codex(cx),
            "attachfile" => self.agent_attach_active_file(self.agent_tab, cx),
            "stage" => {
                let r = self
                    .project
                    .as_ref()
                    .and_then(|p| p.git.root.clone())
                    .ok_or("no repo")?;
                crate::git::stage(&r, arg).map_err(|e| e.to_string())?;
                self.refresh_git(cx);
            }
            "commit" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let i = p.git.commit_input.clone();
                i.update(cx, |i, cx| i.set_value(arg.to_string(), w, cx));
                self.git_commit(w, cx);
            }
            "push" => self.git_push(cx),
            "diff" => {
                let st = self
                    .project
                    .as_ref()
                    .and_then(|p| p.git.status.clone())
                    .ok_or("no status")?;
                let c = st
                    .files
                    .iter()
                    .find(|f| f.path == arg)
                    .ok_or("file not in status")?
                    .clone();
                let staged = c.unstaged.is_none() && c.staged.is_some();
                self.show_file_diff(&c, staged, w, cx);
            }
            "closetab" => self.close_active_tab(w, cx),
            "codexsend" => {
                let i = self
                    .project
                    .as_ref()
                    .ok_or("no project")?
                    .codex
                    .input
                    .clone();
                i.update(cx, |i, cx| i.set_value(arg.to_string(), w, cx));
                self.agent_send(super::agents_panel::AgentTab::Codex, cx);
            }
            "approve" => {
                let c = &mut self.project.as_mut().ok_or("no project")?.codex;
                if c.pending.is_empty() {
                    return Err("nothing pending".into());
                }
                self.agent_decide(
                    super::agents_panel::AgentTab::Codex,
                    0,
                    crate::agents::protocol::Decision::Accept,
                    cx,
                );
            }
            "chattab" => {
                self.settings.layout.codex_visible = true;
                let a = at_agent(arg);
                self.agent_tab = a;
                self.agent_connect(a, cx);
            }
            "chatsend" => {
                let (a, text) = arg.split_once(' ').ok_or("agent text")?;
                let a = at_agent(a);
                let i = self.chat(a).ok_or("no project")?.input.clone();
                i.update(cx, |i, cx| i.set_value(text.to_string(), w, cx));
                self.agent_send(a, cx);
            }
            "chatkeysend" => {
                // Types into the composer and sends with the real Cmd+Enter binding.
                let (a, text) = arg.split_once(' ').ok_or("agent text")?;
                let a = at_agent(a);
                let i = self.chat(a).ok_or("no project")?.input.clone();
                i.update(cx, |i, cx| {
                    i.focus(w, cx);
                    i.set_value(text.to_string(), w, cx);
                });
            }
            "chatstop" => self.agent_stop(at_agent(arg), cx),
            "chatnew" => self.agent_new_chat(at_agent(arg), cx),
            "chathistory" => {
                let (a, on) = arg.split_once(' ').ok_or("agent on|off")?;
                let a = at_agent(a);
                self.chat_mut(a).ok_or("no project")?.show_history = on == "on";
            }
            "chatopen" => {
                let (a, title) = arg.split_once(' ').ok_or("agent title")?;
                let a = at_agent(a);
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                let id = self
                    .sessions
                    .list(a.sessions_key(), &root, title)
                    .first()
                    .map(|e| e.id.clone())
                    .ok_or("no such chat")?;
                self.agent_open_session(a, id, cx);
            }
            "chatopenid" => {
                let (a, id) = arg.split_once(' ').ok_or("agent id")?;
                self.agent_open_session(at_agent(a), id.to_string(), cx);
            }
            "chatdecide" => {
                use crate::agents::protocol::Decision as D;
                let (a, d) = arg.split_once(' ').ok_or("agent decision")?;
                let d = match d {
                    "allow" => D::Accept,
                    "always" => D::AcceptForSession,
                    "decline" => D::Decline,
                    _ => D::Cancel,
                };
                self.agent_decide(at_agent(a), 0, d, cx);
            }
            "chatexpand" => {
                let a = at_agent(arg);
                let c = self.chat_mut(a).ok_or("no project")?;
                let ids: Vec<String> = c
                    .items
                    .iter()
                    .filter_map(|i| match i {
                        super::agent_chat::ChatItem::Action(x) => Some(x.id.clone()),
                        _ => None,
                    })
                    .collect();
                c.expanded.extend(ids);
                c.reload_rows(cx);
            }
            "attach" => {
                let (a, what) = arg.split_once(' ').ok_or("agent what")?;
                let a = at_agent(a);
                match what {
                    "file" => self.agent_attach_active_file(a, cx),
                    "selection" => self.agent_attach_selection(a, cx),
                    _ => self.agent_attach_terminal(a, cx),
                }
            }
            "chatscrolltop" => {
                let c = self.chat(at_agent(arg)).ok_or("no project")?;
                let sc = c.scroller.clone();
                sc.update(cx, |s, cx| {
                    s.scroll_to_item(0, cx);
                });
            }
            "treefocus" => w.focus(&self.tree_focus, cx),
            "treeexpand" => {
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                let path = root.join(arg);
                let p = self.project.as_mut().ok_or("no project")?;
                p.tree.expanded.insert(path.clone());
                self.refresh_tree_dir(&path, cx);
            }
            "treeselect" => {
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                self.project.as_mut().ok_or("no project")?.tree.selected = Some(root.join(arg));
            }
            "clipsave" => {
                self.test_clipboard = cx.read_from_clipboard();
            }
            "cliprestore" => {
                if let Some(item) = self.test_clipboard.take() {
                    cx.write_to_clipboard(item);
                }
            }
            "clipimage" => {
                let bytes = std::fs::read(arg).map_err(|e| e.to_string())?;
                let format = if arg.ends_with(".tiff") {
                    ImageFormat::Tiff
                } else {
                    ImageFormat::Png
                };
                cx.write_to_clipboard(ClipboardItem {
                    entries: vec![ClipboardEntry::Image(Image::from_bytes(format, bytes))],
                });
            }
            "cliptext" => cx.write_to_clipboard(ClipboardItem::new_string(arg.to_string())),
            "composerfocus" => {
                let i = self.chat(at_agent(arg)).ok_or("no project")?.input.clone();
                i.update(cx, |i, cx| i.focus(w, cx));
            }
            "chatattachpath" => {
                let (a, path) = arg.split_once(' ').ok_or("agent path")?;
                self.agent_attach_paths(at_agent(a), vec![path.into()], cx);
            }
            "chatdetach" => {
                let (a, ix) = arg.split_once(' ').ok_or("agent ix")?;
                let c = self.chat_mut(at_agent(a)).ok_or("no project")?;
                c.attachments.remove(ix.parse().map_err(|_| "ix")?);
            }
            "togglebottom" => self.toggle_panel(Dragging::Bottom, w, cx),
            "bottomheight" => {
                self.settings.layout.bottom_height = arg.parse().map_err(|_| "h")?;
                self.project.as_mut().ok_or("no project")?.bottom_height_set = true;
                self.persist();
            }
            "usageclear" => {
                let c = self.chat_mut(at_agent(arg)).ok_or("no project")?;
                c.usage = Default::default();
                c.usage.limits_error = Some("not available".into());
            }
            "chatusage" => {
                let (a, on) = arg.split_once(' ').ok_or("agent on|off")?;
                let a = at_agent(a);
                self.chat_mut(a).ok_or("no project")?.show_usage = on == "on";
                if on == "on" && a == super::agents_panel::AgentTab::Codex {
                    self.codex_refresh_limits(cx);
                }
            }
            "chatmodel" => {
                let (a, id) = arg.split_once(' ').ok_or("agent id")?;
                self.agent_set_model(at_agent(a), id.to_string(), cx);
            }
            "chateffort" => {
                let (a, e) = arg.split_once(' ').ok_or("agent effort")?;
                self.agent_set_effort(at_agent(a), e.to_string(), cx);
            }
            "panelwidth" => {
                self.settings.layout.codex_width = arg.parse().map_err(|_| "width")?;
            }
            "settings" => self.open_settings_tab(w, cx),
            "settingscat" => {
                use super::settings_panel::Category as C;
                self.settings_form.category = match arg {
                    "editor" => C::Editor,
                    "appearance" => C::Appearance,
                    "terminal" => C::Terminal,
                    "languages" => C::Languages,
                    "git" => C::Git,
                    "extensions" => C::Extensions,
                    _ => C::Tools,
                };
            }
            "settheme" => {
                use crate::core::settings::ThemeChoice as T;
                self.set_theme(
                    match arg {
                        "dark" => T::Dark,
                        "light" => T::Light,
                        _ => T::System,
                    },
                    cx,
                );
            }
            "wrap" => {
                self.settings.editor.word_wrap = arg == "on";
                self.apply_editor_settings(w, cx);
            }
            "fontsize" => {
                self.settings.editor.font_size = arg.parse().map_err(|_| "size")?;
                self.apply_editor_settings(w, cx);
            }
            "setlang" => {
                let (id, remember) = arg.split_once(' ').unwrap_or((arg, "file"));
                let l = crate::lang::by_id(id).ok_or("no such language")?;
                self.set_file_language(l, remember == "remember", w, cx);
            }
            "langpicker" => self.open_language_picker(w, cx),
            "selectcommit" => self.select_commit(arg.parse().map_err(|_| "ix")?, cx),
            "commitparent" => self.select_commit_parent(arg.parse().map_err(|_| "ix")?, cx),
            "commitfile" => self.open_commit_file_diff(arg.to_string(), w, cx),
            "graphall" => self.set_graph_all(arg == "on", cx),
            "fetch" => self.git_fetch(cx),
            "window" => return native_window_check(arg),
            "extinstall" => self.install_extension(arg, w, cx),
            "extenable" => self.enable_extension(arg, w, cx),
            "extdisable" => self.turn_off_extension(arg, false, w, cx),
            "extuninstall" => self.turn_off_extension(arg, true, w, cx),
            "extpage" => self.open_extension_page(arg, w, cx),
            "agenttab" => {
                self.settings.layout.codex_visible = true;
            }
            "codexnew" => self.codex_new_chat(cx),
            "codexhistory" => {
                self.project
                    .as_mut()
                    .ok_or("no project")?
                    .codex
                    .show_history = arg == "on";
                self.codex_refresh_history(cx);
            }
            "codexopen" => {
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                let id = self
                    .sessions
                    .list("codex", &root, arg)
                    .first()
                    .map(|e| e.id.clone())
                    .ok_or("no such chat")?;
                self.codex_open_session(id, cx);
            }
            "codexopenid" => self.codex_open_session(arg.to_string(), cx),
            "claudeopen" => self.open_claude(w, cx),
            "extopen" => self.open_agent_from_extension(arg, w, cx),
            "claudeend" => self.claude_end(w, cx),
            "claudetype" => {
                let v = self
                    .project
                    .as_ref()
                    .and_then(|p| p.claude_code.clone())
                    .ok_or("no claude")?;
                v.read(cx).send_text(&arg.replace("\\n", "\n"));
            }
            "renamechat" => {
                let (agent, rest) = arg.split_once(' ').ok_or("agent old|new")?;
                let (old, new) = rest.split_once('|').ok_or("old|new")?;
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                let id = self
                    .sessions
                    .list(agent, &root, old)
                    .first()
                    .map(|e| e.id.clone())
                    .ok_or("no such chat")?;
                self.rename_session(agent, &id, new, cx);
            }
            "media" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let tab = p.active.and_then(|a| p.tabs.get(a)).ok_or("no tab")?;
                let super::editor_area::Tab::Media(m, _) = tab else {
                    return Err("not media".into());
                };
                let m = m.clone();
                m.update(cx, |m, cx| m.test_action(arg, cx));
            }
            "activate" => {
                w.activate_window();
                cx.activate(true);
            }
            "github" => self.open_github_picker(w, cx),
            "closeoverlay" => self.close_overlay(w, cx),
            "clone" => {
                let (url, dest) = arg.split_once(' ').ok_or("clone url dest")?;
                self.start_clone(url.to_string(), dest.into(), w, cx);
            }
            "renameactive" => {
                let path = self.active_file().ok_or("no file")?.path.clone();
                self.project.as_mut().ok_or("no project")?.tree.selected = Some(path);
                self.prompt_rename(w, cx);
            }
            "reviewdiff" => self.agent_open_review(self.agent_tab, w, cx),
            "revertfile" => self.agent_revert_file(self.agent_tab, arg.to_string(), cx),
            "codexstop" => self.codex_stop(cx),
            "write" => {
                // External change to a file, as another program would do.
                let (rel, text) = arg.split_once(' ').ok_or("write path text")?;
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                std::fs::write(root.join(rel), text.replace("\\n", "\n"))
                    .map_err(|e| e.to_string())?;
            }
            "resolve" => {
                let ix = self
                    .project
                    .as_ref()
                    .and_then(|p| p.active)
                    .ok_or("no tab")?;
                match arg {
                    "disk" => self.conflict_load_disk(ix, w, cx),
                    "keep" => self.conflict_keep_buffer(ix, cx),
                    "overwrite" => self.conflict_overwrite(ix, cx),
                    "compare" => self.conflict_diff(ix, w, cx),
                    _ => return Err("resolve?".into()),
                }
            }
            "search" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let i = p.search.input.clone();
                i.update(cx, |i, cx| i.set_value(arg.to_string(), w, cx));
                self.run_project_search(cx);
            }
            "claude" => self.open_claude(w, cx),
            "newterminal" => self.new_terminal(w, cx),
            "promptset" => match &self.overlay {
                Some(super::overlays::Overlay::Prompt { input, .. }) => {
                    let i = input.clone();
                    i.update(cx, |i, cx| i.set_value(arg.to_string(), w, cx));
                }
                _ => return Err("no prompt".into()),
            },
            "submitprompt" => self.submit_prompt(w, cx),
            "treenew" => {
                // treenew <folder|.> file|folder
                let (rel, kind) = arg.split_once(' ').ok_or("dir kind")?;
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                let dir = if rel == "." { root } else { root.join(rel) };
                self.start_new_item(dir, kind == "folder", w, cx);
            }
            "treerename" => {
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                self.start_rename(root.join(arg), w, cx);
            }
            "editset" => {
                let input = self
                    .project
                    .as_ref()
                    .and_then(|p| p.tree.edit.as_ref())
                    .map(|e| e.input.clone())
                    .ok_or("no name field")?;
                input.update(cx, |i, cx| i.set_value(arg.to_string(), w, cx));
            }
            "moveitem" => {
                let (src, dest) = arg.split_once(" -> ").ok_or("src -> dest")?;
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                let dest = if dest == "." {
                    root.clone()
                } else {
                    root.join(dest)
                };
                self.move_item(&root.join(src), &dest, cx);
            }
            "openmoveto" => {
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                self.open_move_to(root.join(arg), w, cx);
            }
            "trash" => {
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                self.request_trash(root.join(arg), cx);
            }
            "confirmchoose" => {
                let action = match &self.overlay {
                    Some(super::overlays::Overlay::Confirm { choices, .. }) => choices
                        .iter()
                        .find(|c| c.label.to_string().contains(arg))
                        .map(|c| c.action.clone())
                        .ok_or("no such choice")?,
                    _ => return Err("no confirmation".into()),
                };
                self.apply_confirm(action, w, cx);
            }
            "assocvalues" => {
                // Types into the File Associations fields; Add is clicked separately.
                let (pat, lang) = arg.split_once(' ').ok_or("pattern language")?;
                let (a, b) = (
                    self.settings_form.assoc_pattern.clone(),
                    self.settings_form.assoc_language.clone(),
                );
                a.update(cx, |i, cx| i.set_value(pat.to_string(), w, cx));
                b.update(cx, |i, cx| i.set_value(lang.to_string(), w, cx));
            }
            "panels" => {
                // Shows the sidebar and the Codex panel without connecting Codex.
                self.settings.layout.sidebar_visible = arg.contains("sidebar");
                self.settings.layout.codex_visible = arg.contains("codex");
            }
            "updatebg" => self.check_for_updates(false, cx),
            "updatelater" => self.updates.visible = false,
            "selectpath" => {
                let p = self.project.as_mut().ok_or("no project")?;
                p.tree.selected = if arg.is_empty() {
                    None
                } else {
                    Some(p.root.join(arg))
                };
            }
            "check" => return self.at_check(arg, w, cx),
            other => return Err(format!("unknown step {other}")),
        }
        cx.notify();
        Ok(String::new())
    }

    fn at_check(
        &mut self,
        cond: &str,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let (what, arg) = cond.split_once(' ').unwrap_or((cond, ""));
        let ok = |b: bool, msg: String| if b { Ok(String::new()) } else { Err(msg) };
        match what {
            "active" => {
                let f = self.active_file().ok_or("no active file")?;
                ok(
                    f.path.ends_with(arg),
                    format!("active is {}", f.path.display()),
                )
            }
            "tabs" => {
                let n = self.project.as_ref().map_or(0, |p| p.tabs.len());
                ok(n.to_string() == arg, format!("{n} tabs"))
            }
            "buffer" => {
                let e = self.at_active_editor()?;
                let t = e.read(cx).text().to_string();
                ok(
                    t.contains(arg),
                    format!("buffer: {:?}", t.chars().take(200).collect::<String>()),
                )
            }
            "notbuffer" => {
                let e = self.at_active_editor()?;
                let t = e.read(cx).text().to_string();
                ok(!t.contains(arg), "buffer still contains it".to_string())
            }
            "dirty" => {
                let f = self.active_file().ok_or("no active file")?;
                ok(
                    (f.dirty as u8).to_string() == arg,
                    format!("dirty={}", f.dirty),
                )
            }
            "conflict" => {
                let f = self.active_file().ok_or("no active file")?;
                ok(
                    (f.conflict.is_some() as u8).to_string() == arg,
                    format!("conflict={}", f.conflict.is_some()),
                )
            }
            "disk" => {
                let (rel, text) = arg.split_once(' ').ok_or("disk rel text")?;
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                let d = std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())?;
                ok(d.contains(text), format!("disk: {d:?}"))
            }
            "notdisk" => {
                let (rel, text) = arg.split_once(' ').ok_or("disk rel text")?;
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                let d = std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())?;
                ok(!d.contains(text), format!("disk: {d:?}"))
            }
            "lsp" => {
                let p = self.project.as_ref().ok_or("no project")?;
                ok(
                    p.lsp_status.get(arg) == Some(&crate::lsp::ServerStatus::Ready),
                    format!("{:?}", p.lsp_status),
                )
            }
            "diagnostics" => {
                let f = self.active_file().ok_or("no active file")?;
                ok(
                    f.diagnostic_counts.0.to_string() == arg,
                    format!("{:?}", f.diagnostic_counts),
                )
            }
            "hasdiagnostics" => {
                let f = self.active_file().ok_or("no active file")?;
                ok(
                    f.diagnostic_counts.0 + f.diagnostic_counts.1 > 0,
                    "none".into(),
                )
            }
            "term" => {
                let t = self.at_terminal()?;
                let s = t.read(cx).screen_text();
                ok(s.contains(arg), format!("screen: {:?}", s.trim_end()))
            }
            "claude" => {
                let c = self
                    .project
                    .as_ref()
                    .and_then(|p| p.claude_code.clone())
                    .ok_or("no claude")?;
                let s = c.read(cx).screen_text();
                ok(s.contains(arg), format!("screen: {:?}", s.trim()))
            }
            "run" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let t = p.run.log.text();
                ok(
                    t.contains(arg),
                    format!(
                        "log: {:?}",
                        t.chars()
                            .rev()
                            .take(300)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect::<String>()
                    ),
                )
            }
            "stopped" => {
                let p = self.project.as_ref().ok_or("no project")?;
                ok(p.run.process.is_none(), "still running".into())
            }
            "configs" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let labels: Vec<_> = p
                    .run
                    .detection
                    .configs
                    .iter()
                    .map(|c| c.label.clone())
                    .collect();
                ok(
                    labels.iter().any(|l| l.contains(arg)),
                    format!("{labels:?}"),
                )
            }
            "searchhits" => {
                let p = self.project.as_ref().ok_or("no project")?;
                ok(
                    !p.search.running && p.search.results.len().to_string() == arg,
                    format!("{} running={}", p.search.results.len(), p.search.running),
                )
            }
            "git" => {
                let st = self
                    .project
                    .as_ref()
                    .and_then(|p| p.git.status.clone())
                    .ok_or("no status")?;
                ok(
                    st.files.iter().any(|f| f.path == arg),
                    format!("{:?}", st.files.iter().map(|f| &f.path).collect::<Vec<_>>()),
                )
            }
            "branch" => {
                let st = self
                    .project
                    .as_ref()
                    .and_then(|p| p.git.status.clone())
                    .ok_or("no status")?;
                ok(
                    st.branch.as_deref() == Some(arg),
                    format!("{:?}", st.branch),
                )
            }
            "gitmsg" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let m = p.git.message.clone().map(|m| m.0).unwrap_or_default();
                ok(
                    p.git.busy.is_none() && m.contains(arg),
                    format!("msg={m:?} busy={:?}", p.git.busy),
                )
            }
            "diffopen" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let tab = p.active.and_then(|a| p.tabs.get(a)).ok_or("no tab")?;
                match tab {
                    Tab::Diff(d) => ok(d.patch.contains(arg), format!("patch: {:?}", d.patch)),
                    _ => Err("active tab is not a diff".into()),
                }
            }
            "edit" => {
                let e = self.project.as_ref().and_then(|p| p.tree.edit.as_ref());
                match (arg, e) {
                    ("none", None) => Ok(String::new()),
                    ("none", Some(_)) => Err("a name field is open".into()),
                    (_, None) => Err("no name field".into()),
                    ("field", Some(_)) => Ok(String::new()),
                    (want, Some(e)) => {
                        let want = want.strip_prefix("error ").unwrap_or(want);
                        let err = e.error.clone().unwrap_or_default();
                        ok(
                            !err.is_empty() && err.contains(want),
                            format!("error={err:?}"),
                        )
                    }
                }
            }
            "editvalue" => {
                let e = self
                    .project
                    .as_ref()
                    .and_then(|p| p.tree.edit.as_ref())
                    .ok_or("no name field")?;
                let v = e.input.read(cx).value().to_string();
                ok(v == arg, format!("value={v:?}"))
            }
            "editfocused" => {
                let e = self
                    .project
                    .as_ref()
                    .and_then(|p| p.tree.edit.as_ref())
                    .ok_or("no name field")?;
                ok(
                    e.input.read(cx).focus_handle(cx).is_focused(_w),
                    "not focused".into(),
                )
            }
            "tabopen" => {
                // tabopen <rel> [dirty]
                let (rel, want_dirty) = match arg.strip_suffix(" dirty") {
                    Some(r) => (r, true),
                    None => (arg, false),
                };
                let p = self.project.as_ref().ok_or("no project")?;
                let path = p.root.join(rel);
                let tab = p
                    .tabs
                    .iter()
                    .find(|t| t.file_path() == Some(path.as_path()));
                let tabs: Vec<String> = p
                    .tabs
                    .iter()
                    .filter_map(|t| t.file_path().map(|f| f.display().to_string()))
                    .collect();
                ok(
                    tab.is_some_and(|t| !want_dirty || t.is_dirty()),
                    format!("tabs={tabs:?}"),
                )
            }
            "notabopen" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let path = p.root.join(arg);
                ok(
                    !p.tabs
                        .iter()
                        .any(|t| t.file_path().is_some_and(|f| f.starts_with(&path))),
                    "still open".into(),
                )
            }
            "confirmbody" => match &self.overlay {
                Some(super::overlays::Overlay::Confirm { body, choices, .. }) => {
                    let labels: Vec<String> = choices.iter().map(|c| c.label.to_string()).collect();
                    ok(body.contains(arg), format!("{body} {labels:?}"))
                }
                _ => Err("no confirmation".into()),
            },
            "hasassoc" => ok(
                self.settings.file_associations.contains_key(arg),
                format!("{:?}", self.settings.file_associations),
            ),
            "contentwidth" => {
                let (op, v) = arg.split_at(1);
                let v: f32 = v.trim().parse().map_err(|_| "number")?;
                let w = self.settings_form.content_width;
                ok(
                    if op == "<" { w < v } else { w >= v },
                    format!("content width {w}"),
                )
            }
            "prompterror" => match &self.overlay {
                Some(super::overlays::Overlay::Prompt { error, .. }) => ok(
                    error.as_deref().is_some_and(|e| e.contains(arg)),
                    format!("error={error:?}"),
                ),
                _ => Err("no prompt".into()),
            },
            "prompttitle" => match &self.overlay {
                Some(super::overlays::Overlay::Prompt {
                    title, description, ..
                }) => {
                    let s = format!("{title} | {description}");
                    ok(s.contains(arg), s)
                }
                _ => Err("no prompt".into()),
            },
            "exists" | "isdir" | "isfile" | "missing" => {
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                let path = root.join(arg);
                let good = match what {
                    "exists" => path.exists(),
                    "isdir" => path.is_dir(),
                    "isfile" => path.is_file(),
                    _ => !path.exists(),
                };
                ok(
                    good,
                    format!(
                        "{} dir={} file={}",
                        path.display(),
                        path.is_dir(),
                        path.is_file()
                    ),
                )
            }
            "toast" => {
                let t = self.toast.as_ref().map(|t| t.0.clone());
                ok(
                    t.as_deref().is_some_and(|t| t.contains(arg)),
                    format!("toast={t:?}"),
                )
            }
            "updatenotice" => {
                let n = self
                    .updates
                    .offer
                    .as_ref()
                    .filter(|_| self.updates.visible)
                    .map(|n| n.version.clone());
                let good = if arg == "none" {
                    n.is_none()
                } else {
                    n.as_deref() == Some(arg)
                };
                ok(good, format!("notice={n:?}"))
            }
            "updateurl" => {
                let n = self.updates.offer.as_ref().map(|n| n.download_url.clone());
                ok(
                    n.as_deref().is_some_and(|u| u.ends_with(arg)),
                    format!("url={n:?}"),
                )
            }
            "updateidle" => ok(!self.updates.checking, "still checking".into()),
            "updatephase" => {
                use super::updates::Phase;
                let phase = self.updates.phase.clone();
                let (name, text) = arg.split_once(' ').unwrap_or((arg, ""));
                let good = match (&phase, name) {
                    (Phase::Offer, "offer") => true,
                    (Phase::Downloading { .. }, "downloading") => true,
                    (Phase::Ready(_), "ready") => true,
                    (Phase::Failed(e), "failed") => e.contains(text),
                    _ => false,
                };
                ok(good, format!("{phase:?}"))
            }
            "version" => ok(
                env!("CARGO_PKG_VERSION") == arg,
                env!("CARGO_PKG_VERSION").into(),
            ),
            "about" => match &self.overlay {
                Some(super::overlays::Overlay::Confirm { title, body, .. }) => {
                    let s = format!("{title}: {body}");
                    ok(title == "About KERN" && body.contains(arg), s)
                }
                _ => Err("About is not open".into()),
            },
            "termtitle" => {
                let t = self.at_terminal()?;
                let title = t.read(cx).title.clone();
                ok(title.contains(arg), title)
            }
            "treerow" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let target = p.root.join(arg);
                let listed = p
                    .tree
                    .children
                    .get(target.parent().ok_or("no parent")?)
                    .is_some_and(|l| l.iter().any(|e| e.path == target));
                ok(listed, "not listed in the tree".into())
            }
            "gitstatus" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let files: Vec<String> = p
                    .git
                    .status
                    .as_ref()
                    .map(|s| s.files.iter().map(|f| f.path.clone()).collect())
                    .unwrap_or_default();
                ok(files.iter().any(|f| f.contains(arg)), format!("{files:?}"))
            }
            "treeselected" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let s = p.tree.selected.as_ref().map(|x| x.display().to_string());
                ok(
                    s.as_deref().is_some_and(|x| x.ends_with(arg)),
                    format!("{s:?}"),
                )
            }
            "treeexpanded" | "treecollapsed" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let open = p.tree.expanded.contains(&p.root.join(arg));
                ok(open == (what == "treeexpanded"), format!("open={open}"))
            }
            "chatconn" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let s = format!("{:?}", p.codex.conn);
                ok(s.contains(arg), s)
            }
            "claudepid" => {
                // One process per project: the session's pid stays the same across opens.
                let p = self.project.as_ref().ok_or("no project")?;
                let v = p.claude_code.clone().ok_or("no claude")?;
                static FIRST: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);
                let id = v.entity_id().as_u64();
                let mut first = FIRST.lock().unwrap();
                let expected = *first.get_or_insert(id);
                ok(
                    expected == id,
                    format!("session {id}, first was {expected}"),
                )
            }
            "chatidle" => {
                let c = self.chat(at_agent(arg)).ok_or("no project")?;
                ok(!c.busy(), "busy".into())
            }
            "chatrunning" => {
                let c = self.chat(at_agent(arg)).ok_or("no project")?;
                ok(c.running, "not running".into())
            }
            "chatpending" => {
                let c = self.chat(at_agent(arg)).ok_or("no project")?;
                ok(!c.pending.is_empty(), "none".into())
            }
            "chatagent" | "chatuser" | "chatnote" | "chaterror" | "chataction" => {
                let (a, text) = arg.split_once(' ').ok_or("agent text")?;
                let c = self.chat(at_agent(a)).ok_or("no project")?;
                use super::agent_chat::ChatItem as I;
                let any = c.items.iter().any(|i| match (what, i) {
                    ("chatagent", I::Agent { text: t, .. })
                    | ("chatuser", I::User { text: t, .. })
                    | ("chatnote", I::Note(t))
                    | ("chaterror", I::Error(t)) => t.contains(text),
                    ("chataction", I::Action(x)) => {
                        format!("{} {:?}", x.title, x.status).contains(text)
                    }
                    _ => false,
                });
                ok(
                    any,
                    format!("{:?}", c.items.iter().rev().take(4).collect::<Vec<_>>()),
                )
            }
            "chatcount" => {
                let (a, n) = arg.split_once(' ').ok_or("agent n")?;
                let c = self.chat(at_agent(a)).ok_or("no project")?;
                ok(c.items.len().to_string() == n, format!("{}", c.items.len()))
            }
            "chatsession" => {
                let c = self.chat(at_agent(arg)).ok_or("no project")?;
                c.session_id.clone().ok_or("no session".to_string())
            }
            "chatchoice" => {
                let (a, want) = arg.split_once(' ').ok_or("agent model|effort")?;
                let (m, e) = self.agent_choice(at_agent(a));
                let got = format!("{m}|{e}");
                ok(got == want, got)
            }
            "composer" => {
                let (a, want) = arg.split_once(' ').ok_or("agent text")?;
                let c = self.chat(at_agent(a)).ok_or("no project")?;
                let v = c.input.read(cx).value().to_string();
                ok(v.replace('\n', "\\n") == want, format!("{v:?}"))
            }
            "chatnotice" => {
                let (a, text) = arg.split_once(' ').ok_or("agent text")?;
                let c = self.chat(at_agent(a)).ok_or("no project")?;
                let n = c.attach_notice.clone();
                ok(
                    n.as_deref().is_some_and(|t| t.contains(text)),
                    format!("{n:?}"),
                )
            }
            "bottomstate" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let s = format!(
                    "visible={} tab={:?} terminals={} height={}",
                    self.settings.layout.bottom_visible,
                    p.bottom_tab,
                    p.terminals.len(),
                    self.settings.layout.bottom_height as i32
                );
                ok(s.contains(arg), s)
            }
            "termfocused" => {
                let t = self.at_terminal()?;
                ok(t.read(cx).is_focused(_w), "not focused".into())
            }
            "composerfocused" => {
                let c = self.chat(at_agent(arg)).ok_or("no project")?;
                ok(
                    c.input.read(cx).focus_handle(cx).is_focused(_w),
                    "composer has no focus".into(),
                )
            }
            "notcomposerfocused" => {
                let c = self.chat(at_agent(arg)).ok_or("no project")?;
                ok(
                    !c.input.read(cx).focus_handle(cx).is_focused(_w),
                    "composer still has focus".into(),
                )
            }
            "overlayfocused" => ok(
                self.overlay_focus.contains_focused(_w, cx),
                "focus is outside the overlay".into(),
            ),
            "notoverlayfocused" => ok(
                !self.overlay_focus.contains_focused(_w, cx),
                "focus is inside the overlay".into(),
            ),
            "queryfocused" => ok(
                self.settings_form
                    .query
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(_w),
                "settings search has no focus".into(),
            ),
            "notqueryfocused" => ok(
                !self
                    .settings_form
                    .query
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(_w),
                "settings search still has focus".into(),
            ),
            "fileexact" => {
                let (file, want) = arg.split_once('|').ok_or("file|text")?;
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                let got = std::fs::read_to_string(root.join(file)).map_err(|e| e.to_string())?;
                let want = want.replace("\\n", "\n").replace("\\t", "\t");
                ok(got == want, format!("{got:?}"))
            }
            "bufferexact" => {
                let e = self.at_active_editor()?;
                let got = e.read(cx).text().to_string();
                let want = arg.replace("\\n", "\n").replace("\\t", "\t");
                ok(got == want, format!("{got:?}"))
            }
            "completionopen" => {
                let e = self.at_active_editor()?;
                ok(
                    e.read(cx).completion_menu_state().open,
                    "menu closed".into(),
                )
            }
            "wrapis" => ok(
                self.settings.editor.word_wrap == (arg == "on"),
                format!("{}", self.settings.editor.word_wrap),
            ),
            "editorfocused" => {
                let e = self.at_active_editor()?;
                let focused = e.read(cx).focus_handle(cx).is_focused(_w);
                ok(focused, "not focused".into())
            }
            "cliconn" => {
                let f = self.extension_facts.get("claude");
                let s = format!("{:?}", f.map(|f| f.signed_in));
                ok(s.contains(arg), s)
            }
            "chatmodels" => {
                let (a, n) = arg.split_once(' ').ok_or("agent id")?;
                let c = self.chat(at_agent(a)).ok_or("no project")?;
                let ids: Vec<String> = c.models.iter().map(|m| m.id.clone()).collect();
                ok(ids.iter().any(|i| i == n), format!("{ids:?}"))
            }
            "chatlimits" => {
                let c = self.chat(at_agent(arg)).ok_or("no project")?;
                let n = c.usage.limits.as_ref().map_or(0, |l| l.buckets.len());
                ok(n > 0, format!("{:?}", c.usage.limits_error))
            }
            "chattokens" => {
                let c = self.chat(at_agent(arg)).ok_or("no project")?;
                let t = c.usage.tokens.clone();
                ok(t.is_some(), "none".into()).map(|_| format!("{t:?}"))
            }
            "chatretry" => {
                let (a, text) = arg.split_once(' ').ok_or("agent text")?;
                let c = self.chat(at_agent(a)).ok_or("no project")?;
                let r = c.retry.as_ref().map(|r| r.text.clone());
                ok(
                    r.as_deref().is_some_and(|t| t.contains(text)),
                    format!("{r:?}"),
                )
            }
            "chatsessionerror" => {
                let (a, text) = arg.split_once(' ').ok_or("agent text")?;
                let c = self.chat(at_agent(a)).ok_or("no project")?;
                let e = c.session_error.as_ref().map(|e| e.message.clone());
                ok(
                    e.as_deref().is_some_and(|m| m.contains(text)),
                    format!("{e:?}"),
                )
            }
            "chatreview" => {
                let (a, path) = arg.split_once(' ').ok_or("agent path")?;
                let c = self.chat(at_agent(a)).ok_or("no project")?;
                let found = c
                    .review
                    .as_ref()
                    .is_some_and(|r| r.files.iter().any(|f| f.path == path));
                ok(found, "not in review".into())
            }
            "chatattachments" => {
                let (a, n) = arg.split_once(' ').ok_or("agent n")?;
                let c = self.chat(at_agent(a)).ok_or("no project")?;
                ok(
                    c.attachments.len().to_string() == n,
                    format!("{}", c.attachments.len()),
                )
            }
            "chatscrolledup" => {
                let c = self.chat(at_agent(arg)).ok_or("no project")?;
                ok(c.scroller.read(cx).is_scrolled_up(), "at the end".into())
            }
            "chatfollowing" => {
                let c = self.chat(at_agent(arg)).ok_or("no project")?;
                ok(
                    c.scroller.read(cx).is_following_tail(),
                    "not following".into(),
                )
            }
            "bottomvisible" => ok(self.settings.layout.bottom_visible, "hidden".into()),
            "bottomhidden" => ok(!self.settings.layout.bottom_visible, "visible".into()),
            "codexconn" => {
                let c = &self.project.as_ref().ok_or("no project")?.codex;
                let s = format!("{:?}", c.conn);
                ok(s.contains(arg), s)
            }
            "codexatt" => {
                let c = &self.project.as_ref().ok_or("no project")?.codex;
                ok(
                    c.attachments.len().to_string() == arg,
                    format!("{}", c.attachments.len()),
                )
            }
            "codexidle" => {
                let c = &self.project.as_ref().ok_or("no project")?.codex;
                ok(!c.running && !c.preparing, "running".into())
            }
            "codexpending" => {
                let c = &self.project.as_ref().ok_or("no project")?.codex;
                ok(!c.pending.is_empty(), "none".into())
            }
            "codextext" => {
                let c = &self.project.as_ref().ok_or("no project")?.codex;
                let any = c.items.iter().any(|i| matches!(i, super::agent_chat::ChatItem::Agent { text, .. } if text.contains(arg)));
                ok(any, format!("{} items", c.items.len()))
            }
            "codexreview" => {
                let c = &self.project.as_ref().ok_or("no project")?.codex;
                let found = c
                    .review
                    .as_ref()
                    .is_some_and(|r| r.files.iter().any(|f| f.path == arg));
                ok(found, format!("{:?}", c.review.as_ref().map(|r| &r.files)))
            }
            "codexrunning" => {
                let c = &self.project.as_ref().ok_or("no project")?.codex;
                ok(c.running, "not running".into())
            }
            "codexnote" => {
                let c = &self.project.as_ref().ok_or("no project")?.codex;
                let any = c
                    .items
                    .iter()
                    .any(|i| matches!(i, super::agent_chat::ChatItem::Note(t) if t.contains(arg)));
                ok(any, format!("{} items", c.items.len()))
            }
            "githubrepos" => match &self.overlay {
                Some(super::overlays::Overlay::Github {
                    view: super::overlays::GithubView::Repos { repos, .. },
                    ..
                }) => ok(!repos.is_empty(), "empty".into()).map(|_| {
                    format!(
                        "{} repositories, {} private",
                        repos.len(),
                        repos.iter().filter(|r| r.is_private).count()
                    )
                }),
                _ => Err("not listed".into()),
            },
            "project" => {
                let p = self.project.as_ref().ok_or("no project")?;
                ok(p.root.ends_with(arg), format!("{}", p.root.display()))
            }
            "focuscheck" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let claude = p
                    .claude_code
                    .as_ref()
                    .is_some_and(|v| v.read(cx).is_focused(_w));
                let term = p.terminals.iter().any(|c| c.read(cx).is_focused(_w));
                ok(
                    arg == "claude" && claude || arg == "term" && term,
                    format!("claude={claude} term={term}"),
                )
            }
            "clipboard" => {
                let t = cx
                    .read_from_clipboard()
                    .and_then(|c| c.text())
                    .unwrap_or_default();
                ok(
                    t.contains(arg),
                    format!("clipboard: {:?}", t.chars().take(120).collect::<String>()),
                )
            }
            "tool" => {
                let (name, want) = arg.split_once(' ').ok_or("tool name state")?;
                let st = crate::platform::tools::locate(name);
                ok(format!("{st:?}").starts_with(want), format!("{st:?}"))
            }
            "env" => {
                let want = arg;
                let p = crate::platform::tools::path_var();
                ok(p.contains(want), p)
            }
            "theme" => ok(
                format!("{:?}", self.settings.theme).eq_ignore_ascii_case(arg),
                format!("{:?}", self.settings.theme),
            ),
            "overlay" => ok(
                self.overlay.is_some() == (arg == "1"),
                format!("overlay={}", self.overlay.is_some()),
            ),
            "mediastate" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let tab = p.active.and_then(|a| p.tabs.get(a)).ok_or("no tab")?;
                let super::editor_area::Tab::Media(m, _) = tab else {
                    return Err("not media".into());
                };
                let s = m.read(cx).describe();
                ok(s.contains(arg), s)
            }
            "mediatab" => {
                let (name, want) = arg.split_once('|').ok_or("mediatab name|text")?;
                let p = self.project.as_ref().ok_or("no project")?;
                let m = p
                    .tabs
                    .iter()
                    .find_map(|t| match t {
                        super::editor_area::Tab::Media(m, path) if path.ends_with(name) => Some(m),
                        _ => None,
                    })
                    .ok_or("no such tab")?;
                let s = m.read(cx).describe();
                ok(s.contains(want), s)
            }
            "ext" => {
                let (id, want) = arg.split_once(' ').ok_or("ext id on|off|none")?;
                let st = self.settings.extension(id);
                let got = if !st.installed {
                    "none"
                } else if st.enabled {
                    "on"
                } else {
                    "off"
                };
                ok(got == want, got.to_string())
            }
            "extfacts" => {
                let (id, want) = arg.split_once(' ').ok_or("extfacts id Some(true)")?;
                let f = self.extension_facts.get(id).ok_or("no facts")?;
                ok(
                    !f.checking && format!("{:?}", f.signed_in) == want,
                    format!("{f:?}"),
                )
            }
            "sessions" => {
                let (agent, n) = arg.split_once(' ').ok_or("sessions agent n")?;
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                let l = self.sessions.list(agent, &root, "");
                ok(
                    l.len().to_string() == n,
                    format!("{:?}", l.iter().map(|e| &e.title).collect::<Vec<_>>()),
                )
            }
            "chatlast" => {
                let (agent, title) = arg.split_once(' ').ok_or("chatlast agent title")?;
                let root = self.project.as_ref().ok_or("no project")?.root.clone();
                let id = self
                    .sessions
                    .get_last_open(agent, &root)
                    .cloned()
                    .unwrap_or_default();
                let got = self
                    .sessions
                    .list(agent, &root, "")
                    .into_iter()
                    .find(|e| e.id == id)
                    .map(|e| e.title);
                ok(
                    got.as_deref().is_some_and(|t| t.contains(title)),
                    format!("{got:?}"),
                )
            }
            "codexerror" => {
                let c = &self.project.as_ref().ok_or("no project")?.codex;
                ok(
                    c.session_error
                        .as_ref()
                        .is_some_and(|e| e.message.contains(arg)),
                    format!("{:?}", c.session_error),
                )
            }
            "codexuser" => {
                let c = &self.project.as_ref().ok_or("no project")?.codex;
                let any = c.items.iter().any(|i| matches!(i, super::agent_chat::ChatItem::User { text, .. } if text.contains(arg)));
                ok(any, format!("{} items", c.items.len()))
            }
            "notcodexuser" => {
                let c = &self.project.as_ref().ok_or("no project")?.codex;
                let any = c.items.iter().any(|i| matches!(i, super::agent_chat::ChatItem::User { text, .. } if text.contains(arg)));
                ok(!any, "still shown".into())
            }
            "codexitems" => {
                let c = &self.project.as_ref().ok_or("no project")?.codex;
                ok(
                    c.items.len().to_string() == arg,
                    format!("{}", c.items.len()),
                )
            }
            "claudealive" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let v = p.claude_code.as_ref().ok_or("no session")?;
                ok(v.read(cx).exited.is_none(), "ended".into())
            }
            "claudeexit" => {
                let p = self.project.as_ref().ok_or("no project")?;
                let v = p.claude_code.as_ref().ok_or("no session")?;
                ok(v.read(cx).exited.is_some(), "running".into())
            }
            "palette" => {
                let has = |this: &Self, name: &str| this.palette_has(name);
                let (want, name) = arg.split_once(' ').ok_or("palette yes|no name")?;
                ok(has(self, name) == (want == "yes"), name.to_string())
            }
            "graph" => {
                let g = &self.project.as_ref().ok_or("no project")?.git.graph;
                let n = g.commits.len();
                let subject = arg.to_string();
                ok(
                    g.commits.iter().any(|c| c.subject.contains(&subject)),
                    format!(
                        "{n} commits: {:?}",
                        g.commits
                            .iter()
                            .take(5)
                            .map(|c| &c.subject)
                            .collect::<Vec<_>>()
                    ),
                )
            }
            "graphcount" => {
                let g = &self.project.as_ref().ok_or("no project")?.git.graph;
                ok(
                    !g.loading && g.commits.len().to_string() == arg,
                    format!("{} loading={}", g.commits.len(), g.loading),
                )
            }
            "commitdetail" => {
                let g = &self.project.as_ref().ok_or("no project")?.git.graph;
                let d = g.detail.as_ref().ok_or("no detail")?;
                ok(
                    d.files.iter().any(|f| f.path == arg),
                    format!("{:?}", d.files),
                )
            }
            "lang" => {
                let f = self.active_file().ok_or("no active file")?;
                ok(f.lang.id == arg, f.lang.id.to_string())
            }
            "probe" => {
                let (name, want) = arg.split_once(' ').ok_or("probe tool state")?;
                let got = self.settings_form.probes.get(name);
                ok(format!("{got:?}").starts_with(want), format!("{got:?}"))
            }
            "tree" => {
                let p = self.project.as_ref().ok_or("no project")?;
                ok(
                    p.tree
                        .children
                        .get(&p.root)
                        .is_some_and(|l| l.iter().any(|e| e.name == arg)),
                    "not in tree".into(),
                )
            }
            other => Err(format!("unknown check {other}")),
        }
    }

    fn at_completion(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Task<anyhow::Result<Vec<String>>>> {
        let e = self.active_file()?.editor.clone();
        let (provider, rope, offset): (
            std::rc::Rc<dyn gpui_kit::component::input::CompletionProvider>,
            Rope,
            usize,
        ) = {
            let r = e.read(cx);
            (
                r.lsp().completion_provider.clone()?,
                r.text().clone(),
                r.cursor(),
            )
        };
        let ctx = lsp_types::CompletionContext {
            trigger_kind: lsp_types::CompletionTriggerKind::INVOKED,
            trigger_character: None,
        };
        let task = provider.completions(&rope, offset, ctx, w, cx);
        Some(cx.background_spawn(async move {
            let res = task.await?;
            Ok(match res {
                lsp_types::CompletionResponse::Array(a) => a.into_iter().map(|i| i.label).collect(),
                lsp_types::CompletionResponse::List(l) => {
                    l.items.into_iter().map(|i| i.label).collect()
                }
            })
        }))
    }

    fn at_hover(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Task<anyhow::Result<Option<String>>>> {
        let e = self.active_file()?.editor.clone();
        let (provider, rope, offset) = {
            let r = e.read(cx);
            (
                r.lsp().hover_provider.clone()?,
                r.text().clone(),
                r.cursor(),
            )
        };
        let task = provider.hover(&rope, offset, w, cx);
        Some(cx.background_spawn(async move {
            Ok(task
                .await?
                .map(|h| serde_json::to_string(&h.contents).unwrap_or_default()))
        }))
    }
}
