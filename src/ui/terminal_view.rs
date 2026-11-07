//! Terminal view: renders the grid, sends keyboard and mouse input to the PTY.
use super::theme::{Palette, pal};
use crate::run::output::find_urls;
use crate::terminal::input::{Mods, key_to_bytes, paste_bytes};
use crate::terminal::{SessionEvent, TermSize, TerminalSession};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as TermPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color as AColor, CursorShape, NamedColor};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

gpui_kit::actions!(
    terminal,
    [
        TermCopy,
        TermPaste,
        TermClear,
        TermSelectAll,
        TermTab,
        TermBackTab,
        TermCopyOrInterrupt
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-c", TermCopy, Some("Terminal")),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-v", TermPaste, Some("Terminal")),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-k", TermClear, Some("Terminal")),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-a", TermSelectAll, Some("Terminal")),
        // Windows terminals: Ctrl+C copies a selection and interrupts otherwise;
        // Ctrl+Shift+C/V and Ctrl+V as in Windows Terminal.
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-c", TermCopyOrInterrupt, Some("Terminal")),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-c", TermCopy, Some("Terminal")),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-v", TermPaste, Some("Terminal")),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-v", TermPaste, Some("Terminal")),
        // The window root binds Tab to focus traversal, and bindings run before key
        // listeners. A deeper binding keeps Tab with the shell while a terminal has focus.
        KeyBinding::new("tab", TermTab, Some("Terminal")),
        KeyBinding::new("shift-tab", TermBackTab, Some("Terminal")),
    ]);
}

pub struct TerminalView {
    pub session: Option<Arc<TerminalSession>>,
    pub title: String,
    pub base_title: String,
    pub exited: Option<Option<i32>>,
    pub exited_at: Option<std::time::Instant>,
    pub error: Option<String>,
    focus: FocusHandle,
    program: Option<(String, Vec<String>)>,
    cwd: PathBuf,
    shell: Option<String>,
    cell: Size<Pixels>,
    origin: Point<Pixels>,
    marked: String,
    hovered_link: Option<HoveredLink>,
    selecting: bool,
    mouse_moved: bool,
    scroll_remainder: f32,
    last_size: Option<TermSize>,
}

#[derive(Clone, PartialEq)]
struct HoveredLink {
    url: String,
    line: i32,
    cols: Range<usize>,
}

impl TerminalView {
    pub fn new(
        title: impl Into<String>,
        program: Option<(String, Vec<String>)>,
        cwd: &Path,
        shell: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = title.into();
        Self {
            session: None,
            title: title.clone(),
            base_title: title,
            exited: None,
            exited_at: None,
            error: None,
            focus: cx.focus_handle(),
            program,
            cwd: cwd.to_path_buf(),
            shell,
            cell: size(px(8.), px(16.)),
            origin: Point::default(),
            marked: String::new(),
            hovered_link: None,
            selecting: false,
            mouse_moved: false,
            scroll_remainder: 0.,
            last_size: None,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    fn start_session(&mut self, sz: TermSize, cx: &mut Context<Self>) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        match TerminalSession::spawn(
            self.program.clone(),
            &self.cwd,
            self.shell.as_deref(),
            sz,
            tx,
        ) {
            Ok(s) => {
                self.session = Some(Arc::new(s));
                self.exited = None;
                self.error = None;
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                return;
            }
        }
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            while let Some(ev) = rx.next().await {
                // Coalesce bursts of output into one repaint.
                let mut events = vec![ev];
                while let Ok(more) = rx.try_recv() {
                    events.push(more);
                }
                let ok = this.update(cx, |this, cx| {
                    for ev in events {
                        match ev {
                            SessionEvent::Wakeup | SessionEvent::Bell => {}
                            SessionEvent::Title(t) => {
                                this.title = if t.is_empty() {
                                    this.base_title.clone()
                                } else {
                                    t
                                }
                            }
                            SessionEvent::ClipboardStore(s) => {
                                cx.write_to_clipboard(ClipboardItem::new_string(s))
                            }
                            SessionEvent::Exited(code) => {
                                this.exited = Some(code);
                                this.exited_at = Some(std::time::Instant::now());
                            }
                        }
                    }
                    cx.notify();
                });
                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Test and automation hook: send raw text to the program.
    pub fn send_text(&self, text: &str) {
        if let Some(s) = &self.session {
            s.write(text.as_bytes().to_vec());
        }
    }

    pub fn screen_text(&self) -> String {
        let Some(s) = &self.session else {
            return String::new();
        };
        let term = s.term.lock();
        let mut out = String::new();
        for l in 0..term.screen_lines() {
            for c in 0..term.columns() {
                out.push(term.grid()[Line(l as i32)][Column(c)].c);
            }
            out.push('\n');
        }
        out
    }

    fn write(&self, bytes: Vec<u8>) {
        if let Some(s) = &self.session {
            let at_bottom = s.term.lock().grid().display_offset() == 0;
            if !at_bottom {
                s.term.lock().scroll_display(Scroll::Bottom);
            }
            s.write(bytes);
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = k.modifiers;
        if m.platform {
            return; // Cmd shortcuts are application actions, never terminal input.
        }
        let app_cursor = self
            .session
            .as_ref()
            .is_some_and(|s| s.term.lock().mode().contains(TermMode::APP_CURSOR));
        // Option with a character is text input on macOS unless it produces nothing.
        if m.alt
            && !m.control
            && k.key_char.as_ref().is_some_and(|c| !c.is_empty())
            && k.key.chars().count() == 1
        {
            return;
        }
        let mods = Mods {
            ctrl: m.control,
            alt: m.alt,
            shift: m.shift,
        };
        if let Some(bytes) = key_to_bytes(&k.key, mods, app_cursor) {
            self.send_key_bytes(bytes, cx);
            cx.stop_propagation();
        }
    }

    fn send_key_bytes(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if let Some(s) = &self.session {
            s.term.lock().selection = None;
        }
        self.write(bytes);
        cx.notify();
    }

    fn tab(&mut self, _: &TermTab, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(bytes) = key_to_bytes("tab", Mods::default(), false) {
            self.send_key_bytes(bytes, cx);
        }
    }

    fn back_tab(&mut self, _: &TermBackTab, _w: &mut Window, cx: &mut Context<Self>) {
        let mods = Mods {
            shift: true,
            ..Mods::default()
        };
        if let Some(bytes) = key_to_bytes("tab", mods, false) {
            self.send_key_bytes(bytes, cx);
        }
    }

    fn grid_point(&self, pos: Point<Pixels>) -> Option<(TermPoint, Side)> {
        let s = self.session.as_ref()?;
        let term = s.term.lock();
        let x = (f32::from(pos.x - self.origin.x) / f32::from(self.cell.width)).max(0.);
        let y = (f32::from(pos.y - self.origin.y) / f32::from(self.cell.height)).max(0.);
        let col = (x as usize).min(term.columns().saturating_sub(1));
        let row = (y as usize).min(term.screen_lines().saturating_sub(1));
        let side = if x.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        let offset = term.grid().display_offset() as i32;
        Some((TermPoint::new(Line(row as i32 - offset), Column(col)), side))
    }

    fn mouse_reporting(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.term.lock().mode().intersects(TermMode::MOUSE_MODE))
    }

    fn report_mouse(&self, button: u8, pos: Point<Pixels>, press: bool, mods: Modifiers) {
        let Some((pt, _)) = self.grid_point(pos) else {
            return;
        };
        let Some(s) = &self.session else { return };
        let mut code = button;
        if mods.shift {
            code += 4;
        }
        if mods.alt {
            code += 8;
        }
        if mods.control {
            code += 16;
        }
        let (col, line) = (pt.column.0 + 1, pt.line.0 + 1);
        let sgr = s.term.lock().mode().contains(TermMode::SGR_MOUSE);
        if sgr {
            s.write(
                format!("\x1b[<{code};{col};{line}{}", if press { 'M' } else { 'm' }).into_bytes(),
            );
        } else if press {
            let enc = |v: usize| (v + 32).min(255) as u8;
            s.write(vec![
                0x1b,
                b'[',
                b'M',
                code + 32,
                enc(col),
                enc(line.max(0) as usize),
            ]);
        }
    }

    /// The URL under a grid point: explicit OSC 8 links first, then plain text,
    /// joining soft-wrapped rows so that long sign-in URLs stay in one piece.
    fn link_at(&self, pt: TermPoint) -> Option<HoveredLink> {
        let s = self.session.as_ref()?;
        let term = s.term.lock();
        let grid = term.grid();
        let cols = term.columns();
        if pt.line.0 >= term.screen_lines() as i32 || pt.line.0 < -(grid.history_size() as i32) {
            return None;
        }
        if let Some(h) = grid[pt.line][pt.column].hyperlink() {
            return Some(HoveredLink {
                url: h.uri().to_string(),
                line: pt.line.0,
                cols: pt.column.0..pt.column.0 + 1,
            });
        }
        let top = -(grid.history_size() as i32);
        let mut first = pt.line.0;
        while first > top
            && grid[Line(first - 1)][Column(cols - 1)]
                .flags
                .contains(Flags::WRAPLINE)
        {
            first -= 1;
        }
        let mut text = String::new();
        let mut byte_cells: Vec<(i32, usize)> = vec![];
        let mut l = first;
        loop {
            for c in 0..cols {
                let cell = &grid[Line(l)][Column(c)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                for _ in 0..cell.c.len_utf8() {
                    byte_cells.push((l, c));
                }
                text.push(cell.c);
            }
            if l + 1 < term.screen_lines() as i32
                && grid[Line(l)][Column(cols - 1)]
                    .flags
                    .contains(Flags::WRAPLINE)
            {
                l += 1;
            } else {
                break;
            }
        }
        for link in find_urls(&text) {
            let start = byte_cells.get(link.start).copied()?;
            let end = byte_cells.get(link.end.saturating_sub(1)).copied()?;
            let inside = (pt.line.0, pt.column.0) >= start && (pt.line.0, pt.column.0) <= end;
            if inside {
                let (a, b) = if start.0 == pt.line.0 {
                    (start.1, cols)
                } else {
                    (0, cols)
                };
                let b = if end.0 == pt.line.0 { end.1 + 1 } else { b };
                return Some(HoveredLink {
                    url: link.url,
                    line: pt.line.0,
                    cols: a..b,
                });
            }
        }
        None
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus(window, cx);
        if self.mouse_reporting() && !ev.modifiers.shift {
            self.report_mouse(0, ev.position, true, ev.modifiers);
            return;
        }
        let Some((pt, side)) = self.grid_point(ev.position) else {
            return;
        };
        let Some(s) = self.session.clone() else {
            return;
        };
        let ty = match ev.click_count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        s.term.lock().selection = Some(Selection::new(ty, pt, side));
        self.selecting = true;
        self.mouse_moved = false;
        cx.notify();
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _w: &mut Window, cx: &mut Context<Self>) {
        if self.mouse_reporting() && !ev.modifiers.shift {
            self.report_mouse(0, ev.position, false, ev.modifiers);
            return;
        }
        if self.selecting && !self.mouse_moved {
            if let Some(s) = &self.session {
                let keep = s
                    .term
                    .lock()
                    .selection
                    .as_ref()
                    .is_some_and(|sel| !matches!(sel.ty, SelectionType::Simple));
                if !keep {
                    s.term.lock().selection = None;
                }
            }
            if ev.click_count == 1
                && let Some((pt, _)) = self.grid_point(ev.position)
                && let Some(link) = self.link_at(pt)
            {
                cx.open_url(&link.url);
            }
        }
        self.selecting = false;
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let Some((pt, side)) = self.grid_point(ev.position) else {
            return;
        };
        if self.selecting && ev.dragging() {
            self.mouse_moved = true;
            if let Some(s) = &self.session
                && let Some(sel) = s.term.lock().selection.as_mut()
            {
                sel.update(pt, side);
            }
            cx.notify();
            return;
        }
        if ev.dragging() {
            return;
        }
        let link = self.link_at(pt);
        if link != self.hovered_link {
            self.hovered_link = link;
            cx.notify();
        }
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.session.clone() else {
            return;
        };
        let delta = ev.delta.pixel_delta(self.cell.height);
        self.scroll_remainder += f32::from(delta.y);
        let lines = (self.scroll_remainder / f32::from(self.cell.height)) as i32;
        if lines == 0 {
            return;
        }
        self.scroll_remainder -= lines as f32 * f32::from(self.cell.height);
        let mode = *s.term.lock().mode();
        if mode.intersects(TermMode::MOUSE_MODE) {
            let button = if lines > 0 { 64 } else { 65 };
            for _ in 0..lines.abs().min(10) {
                self.report_mouse(button, ev.position, true, ev.modifiers);
            }
        } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            let seq: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            s.write(seq.repeat(lines.unsigned_abs() as usize));
        } else {
            s.term.lock().scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }

    fn copy(&mut self, _: &TermCopy, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = &self.session
            && let Some(text) = s.term.lock().selection_to_string()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Ctrl+C: copy when text is selected, otherwise the interrupt the shell expects.
    fn copy_or_interrupt(
        &mut self,
        _: &TermCopyOrInterrupt,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self
            .session
            .as_ref()
            .and_then(|s| s.term.lock().selection_to_string())
            .filter(|t| !t.is_empty());
        match selected {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                if let Some(s) = &self.session {
                    s.term.lock().selection = None;
                }
                cx.notify();
            }
            None => {
                let _ = w;
                self.send_key_bytes(vec![0x03], cx);
            }
        }
    }

    fn paste(&mut self, _: &TermPaste, _w: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = item.text() else { return };
        let bracketed = self
            .session
            .as_ref()
            .is_some_and(|s| s.term.lock().mode().contains(TermMode::BRACKETED_PASTE));
        self.write(paste_bytes(&text, bracketed));
    }

    fn clear(&mut self, _: &TermClear, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = &self.session {
            s.term.lock().grid_mut().clear_history();
            s.write(vec![0x0c]);
        }
        cx.notify();
    }

    fn select_all(&mut self, _: &TermSelectAll, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = &self.session {
            let mut term = s.term.lock();
            let top = term.topmost_line();
            let bottom = term.bottommost_line();
            let cols = term.columns();
            let mut sel = Selection::new(
                SelectionType::Simple,
                TermPoint::new(top, Column(0)),
                Side::Left,
            );
            sel.update(TermPoint::new(bottom, Column(cols - 1)), Side::Right);
            term.selection = Some(sel);
        }
        cx.notify();
    }
}

fn named(n: NamedColor, p: &Palette) -> Hsla {
    let h = |x: u32| -> Hsla { rgb(x).into() };
    use NamedColor::*;
    let dark = p.dark;
    match n {
        Foreground | BrightForeground | DimForeground => p.fg,
        Background => p.bg,
        Cursor => p.accent,
        Black | DimBlack => h(if dark { 0x2a2a31 } else { 0x25252b }),
        Red | DimRed => h(if dark { 0xf2777a } else { 0xc93c3f }),
        Green | DimGreen => h(if dark { 0x8bd49c } else { 0x2e8b57 }),
        Yellow | DimYellow => h(if dark { 0xe6c07b } else { 0xa66f00 }),
        Blue | DimBlue => h(if dark { 0x82aaff } else { 0x3567c6 }),
        Magenta | DimMagenta => h(if dark { 0xc5b4f0 } else { 0x7a5fd6 }),
        Cyan | DimCyan => h(if dark { 0x7fdbca } else { 0x13808a }),
        White | DimWhite => h(if dark { 0xcfcfd6 } else { 0x6b6b76 }),
        BrightBlack => h(if dark { 0x6b6b76 } else { 0x8e8e99 }),
        BrightRed => h(if dark { 0xff8f91 } else { 0xe05558 }),
        BrightGreen => h(if dark { 0xa8e6b8 } else { 0x35a269 }),
        BrightYellow => h(if dark { 0xf5d394 } else { 0xc48500 }),
        BrightBlue => h(if dark { 0xa0c0ff } else { 0x4c7fe0 }),
        BrightMagenta => h(if dark { 0xd9ccf8 } else { 0x9276e6 }),
        BrightCyan => h(if dark { 0x9df0e0 } else { 0x1aa0ac }),
        BrightWhite => h(if dark { 0xffffff } else { 0x25252b }),
    }
}

fn color(c: AColor, p: &Palette) -> Hsla {
    match c {
        AColor::Named(n) => named(n, p),
        AColor::Spec(rgb_) => Rgba {
            r: rgb_.r as f32 / 255.,
            g: rgb_.g as f32 / 255.,
            b: rgb_.b as f32 / 255.,
            a: 1.,
        }
        .into(),
        AColor::Indexed(i) => indexed(i, p),
    }
}

fn indexed(i: u8, p: &Palette) -> Hsla {
    use NamedColor::*;
    const BASE: [NamedColor; 16] = [
        Black,
        Red,
        Green,
        Yellow,
        Blue,
        Magenta,
        Cyan,
        White,
        BrightBlack,
        BrightRed,
        BrightGreen,
        BrightYellow,
        BrightBlue,
        BrightMagenta,
        BrightCyan,
        BrightWhite,
    ];
    match i {
        0..=15 => named(BASE[i as usize], p),
        16..=231 => {
            let v = i - 16;
            let f = |c: u8| {
                if c == 0 {
                    0.
                } else {
                    (55 + c as u32 * 40) as f32 / 255.
                }
            };
            Rgba {
                r: f(v / 36),
                g: f((v / 6) % 6),
                b: f(v % 6),
                a: 1.,
            }
            .into()
        }
        _ => {
            let g = (8 + (i as u32 - 232) * 10) as f32 / 255.;
            Rgba {
                r: g,
                g,
                b: g,
                a: 1.,
            }
            .into()
        }
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _r: Range<usize>,
        _a: &mut Option<Range<usize>>,
        _w: &mut Window,
        _c: &mut Context<Self>,
    ) -> Option<String> {
        None
    }
    fn selected_text_range(
        &mut self,
        _i: bool,
        _w: &mut Window,
        _c: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }
    fn marked_text_range(&self, _w: &mut Window, _c: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.marked.is_empty()).then(|| 0..self.marked.encode_utf16().count())
    }
    fn unmark_text(&mut self, _w: &mut Window, cx: &mut Context<Self>) {
        self.marked.clear();
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        _r: Option<Range<usize>>,
        text: &str,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked.clear();
        if let Some(s) = &self.session {
            s.term.lock().selection = None;
        }
        self.write(text.as_bytes().to_vec());
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _r: Option<Range<usize>>,
        text: &str,
        _m: Option<Range<usize>>,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = text.to_string();
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _r: Range<usize>,
        _b: Bounds<Pixels>,
        _w: &mut Window,
        _c: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let s = self.session.as_ref()?;
        let term = s.term.lock();
        let cur = term.grid().cursor.point;
        Some(Bounds::new(
            point(
                self.origin.x + self.cell.width * cur.column.0 as f32,
                self.origin.y + self.cell.height * cur.line.0 as f32,
            ),
            self.cell,
        ))
    }
    fn character_index_for_point(
        &mut self,
        _p: Point<Pixels>,
        _w: &mut Window,
        _c: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

fn is_uniform(a: &CellStyle, b: &CellStyle) -> bool {
    a == b
}

#[derive(Clone, PartialEq)]
struct CellStyle {
    fg: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

struct Run {
    col: usize,
    text: String,
    style: CellStyle,
}

impl Render for TerminalView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let entity = cx.entity();
        let focus = self.focus.clone();
        let exited = self.exited;
        let error = self.error.clone();
        let hover_link = self.hovered_link.is_some();
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(p.bg)
            .when(hover_link, |d| d.cursor_pointer())
            .when(!hover_link, |d| d.cursor_text())
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::copy_or_interrupt))
            .on_action(cx.listener(Self::back_tab))
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll(cx.listener(Self::on_scroll))
            .child(
                canvas(
                    {
                        let entity = entity.clone();
                        move |bounds, window, cx| {
                            let font = Font {
                                family: "Menlo".into(),
                                features: FontFeatures::disable_ligatures(),
                                fallbacks: None,
                                weight: FontWeight::NORMAL,
                                style: FontStyle::Normal,
                            };
                            let font_size = px(cx.global::<super::theme::TermPrefs>().font_size);
                            let font_id = window.text_system().resolve_font(&font);
                            let width = window
                                .text_system()
                                .advance(font_id, font_size, 'M')
                                .map(|s| s.width)
                                .unwrap_or(px(8.));
                            let height = (font_size * 1.28).round();
                            let cell = size(width, height);
                            entity.update(cx, |this, cx| {
                                this.cell = cell;
                                this.origin = bounds.origin + point(px(6.), px(2.));
                                let cols = (f32::from(bounds.size.width - px(12.))
                                    / f32::from(width))
                                .floor()
                                .max(2.) as u16;
                                let lines = (f32::from(bounds.size.height - px(4.))
                                    / f32::from(height))
                                .floor()
                                .max(1.) as u16;
                                let sz = TermSize {
                                    cols,
                                    lines,
                                    cell_width: f32::from(width) as u16,
                                    cell_height: f32::from(height) as u16,
                                };
                                if this.session.is_none()
                                    && this.error.is_none()
                                    && this.exited.is_none()
                                {
                                    this.start_session(sz, cx);
                                } else if this.last_size != Some(sz)
                                    && let Some(s) = &this.session
                                {
                                    s.resize(sz);
                                }
                                this.last_size = Some(sz);
                            });
                            font
                        }
                    },
                    move |bounds, font, window, cx| {
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, entity.clone()),
                            cx,
                        );
                        let view = entity.read(cx);
                        let Some(session) = view.session.clone() else {
                            return;
                        };
                        let (origin, cell, marked, focused) = (
                            view.origin,
                            view.cell,
                            view.marked.clone(),
                            view.focus.is_focused(window),
                        );
                        let hovered = view.hovered_link.clone();
                        paint_grid(
                            &session, origin, cell, font, &marked, focused, hovered, window, cx,
                        );
                    },
                )
                .size_full(),
            )
            .when_some(exited, |d, code| {
                d.child(
                    div()
                        .absolute()
                        .bottom(px(4.))
                        .right(px(10.))
                        .text_size(px(11.))
                        .text_color(p.muted)
                        .child(match code {
                            Some(c) => format!("process exited with code {c}"),
                            None => "process exited".to_string(),
                        }),
                )
            })
            .when_some(error, |d, e| {
                d.child(
                    div()
                        .absolute()
                        .top(px(8.))
                        .left(px(8.))
                        .text_size(px(12.))
                        .text_color(p.danger)
                        .child(e),
                )
            })
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_grid(
    session: &TerminalSession,
    origin: Point<Pixels>,
    cell: Size<Pixels>,
    font: Font,
    marked: &str,
    focused: bool,
    hovered: Option<HoveredLink>,
    window: &mut Window,
    cx: &mut App,
) {
    let p = pal(cx);
    let font_size = px(cx.global::<super::theme::TermPrefs>().font_size);
    let mut bg_rects: Vec<(usize, usize, usize, Hsla)> = vec![];
    let mut rows: Vec<Vec<Run>> = vec![];
    let cursor;
    {
        let term = session.term.lock();
        let content = term.renderable_content();
        let offset = content.display_offset as i32;
        let selection = content.selection;
        let lines = term.screen_lines();
        rows.resize_with(lines, Vec::new);
        cursor = content.cursor;
        for ic in content.display_iter {
            let row = (ic.point.line.0 + offset) as usize;
            if row >= lines {
                continue;
            }
            let c = ic.cell;
            if c.flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let (mut fg, mut bg) = (color(c.fg, &p), c.bg);
            let selected = selection.as_ref().is_some_and(|s| s.contains(ic.point));
            let mut bg_color = if matches!(bg, AColor::Named(NamedColor::Background)) {
                None
            } else {
                Some(color(bg, &p))
            };
            if c.flags.contains(Flags::INVERSE) {
                let old = bg_color.unwrap_or(p.bg);
                bg_color = Some(fg);
                fg = old;
                bg = AColor::Named(NamedColor::Foreground);
            }
            let _ = bg;
            if c.flags.contains(Flags::DIM) {
                fg.a *= 0.65;
            }
            if c.flags.contains(Flags::HIDDEN) {
                fg.a = 0.;
            }
            let col = ic.point.column.0;
            let width = if c.flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            };
            if selected {
                bg_rects.push((row, col, width, p.selection));
            } else if let Some(b) = bg_color {
                bg_rects.push((row, col, width, b));
            }
            let ch = c.c;
            if ch == ' ' || ch == '\0' {
                continue;
            }
            let link_hover = hovered
                .as_ref()
                .is_some_and(|h| h.line + offset == row as i32 && h.cols.contains(&col));
            let style = CellStyle {
                fg,
                bold: c.flags.contains(Flags::BOLD),
                italic: c.flags.contains(Flags::ITALIC),
                underline: c.flags.intersects(Flags::ALL_UNDERLINES) || link_hover,
                strike: c.flags.contains(Flags::STRIKEOUT),
            };
            let mut text = String::new();
            text.push(ch);
            if let Some(extra) = c.zerowidth() {
                text.extend(extra.iter());
            }
            let current = &mut rows[row];
            // Join only directly adjacent narrow cells so that every run starts on the grid.
            if let Some(last) = current.last_mut()
                && is_uniform(&last.style, &style)
                && last.col + last.text.chars().count() == col
                && width == 1
                && last.text.is_ascii()
                && ch.is_ascii()
            {
                last.text.push_str(&text);
                continue;
            }
            current.push(Run { col, text, style });
        }
    }
    let at = |row: usize, col: usize| {
        point(
            origin.x + cell.width * col as f32,
            origin.y + cell.height * row as f32,
        )
    };
    for (row, col, width, color) in bg_rects {
        window.paint_quad(fill(
            Bounds::new(at(row, col), size(cell.width * width as f32, cell.height)),
            color,
        ));
    }
    for (row, runs) in rows.into_iter().enumerate() {
        for run in runs {
            let mut f = font.clone();
            if run.style.bold {
                f.weight = FontWeight::BOLD;
            }
            if run.style.italic {
                f.style = FontStyle::Italic;
            }
            let tr = TextRun {
                len: run.text.len(),
                font: f,
                color: run.style.fg,
                background_color: None,
                underline: run.style.underline.then(|| UnderlineStyle {
                    thickness: px(1.),
                    color: Some(run.style.fg),
                    wavy: false,
                }),
                strikethrough: run.style.strike.then(|| StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(run.style.fg),
                }),
            };
            let line = window
                .text_system()
                .shape_line(run.text.into(), font_size, &[tr], None);
            let _ = line.paint(
                at(row, run.col),
                cell.height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
    }
    // Cursor and IME composition.
    let (cline, ccol) = (cursor.point.line.0.max(0) as usize, cursor.point.column.0);
    if !marked.is_empty() {
        let tr = TextRun {
            len: marked.len(),
            font: font.clone(),
            color: p.accent,
            background_color: None,
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(p.accent),
                wavy: false,
            }),
            strikethrough: None,
        };
        let line =
            window
                .text_system()
                .shape_line(marked.to_string().into(), font_size, &[tr], None);
        let _ = line.paint(
            at(cline, ccol),
            cell.height,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    } else if !matches!(cursor.shape, CursorShape::Hidden) {
        let b = Bounds::new(at(cline, ccol), cell);
        if !focused {
            window.paint_quad(outline(b, p.accent, BorderStyle::Solid));
        } else {
            match cursor.shape {
                CursorShape::Beam => {
                    window.paint_quad(fill(Bounds::new(b.origin, size(px(2.), cell.height)), p.fg))
                }
                CursorShape::Underline => window.paint_quad(fill(
                    Bounds::new(
                        point(b.origin.x, b.origin.y + cell.height - px(2.)),
                        size(cell.width, px(2.)),
                    ),
                    p.accent,
                )),
                _ => {
                    let mut c = p.accent;
                    c.a = 0.55;
                    window.paint_quad(fill(b, c));
                }
            }
        }
    }
}
