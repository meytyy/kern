//! Graphite, a neutral dark grey theme, and the light Cream theme.
use crate::core::settings::{Settings, ThemeChoice};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct Palette {
    /// The window shell.
    pub bg: Hsla,
    /// Where content is read: editor, terminal, Settings, full-page views.
    pub editor: Hsla,
    pub panel: Hsla,
    pub bar: Hsla,
    pub hover: Hsla,
    pub active: Hsla,
    pub border: Hsla,
    pub fg: Hsla,
    pub muted: Hsla,
    /// Coral that stays readable as text, icons and focus rings on this theme.
    pub accent: Hsla,
    /// True Coral, for filled controls and indicators. Text on it is `accent_fg`.
    pub accent_fill: Hsla,
    pub accent_fill_hover: Hsla,
    pub accent_fill_active: Hsla,
    pub accent_fg: Hsla,
    /// A quiet selected-row tint. Not Coral: selection must not compete with code.
    pub accent_soft: Hsla,
    pub danger: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub selection: Hsla,
    /// Raised surfaces: cards, composer.
    pub surface: Hsla,
    pub user_bubble: Hsla,
    pub tint_blue: Hsla,
    pub tint_green: Hsla,
    pub tint_yellow: Hsla,
    pub tint_orange: Hsla,
    pub tint_purple: Hsla,
    pub tint_folder: Hsla,
    pub tint_cyan: Hsla,
    pub tint_red: Hsla,
    pub tint_pink: Hsla,
    pub dark: bool,
}

impl Global for Palette {}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex: u32, alpha: f32) -> Hsla {
    let mut h: Hsla = rgb(hex).into();
    h.a = alpha;
    h
}

/// Graphite: neutral greys with no hue in the chrome. Focus and selection are a
/// light grey outline or tint; colour is left to syntax, errors and Git.
pub fn dark() -> Palette {
    let accent = c(0xd4d7dc);
    Palette {
        bg: c(0x191b1e),
        editor: c(0x191b1e),
        panel: c(0x222428),
        bar: c(0x222428),
        hover: c(0x2c2f34),
        active: c(0x363940),
        border: c(0x30333a),
        fg: c(0xe7e9ec),
        // 6.6:1 on the editor, 5.6:1 on panels.
        muted: c(0xa0a6ae),
        accent,
        accent_fill: c(0xd4d7dc),
        accent_fill_hover: c(0xe7e9ec),
        accent_fill_active: c(0xbfc3c9),
        accent_fg: c(0x191b1e),
        accent_soft: ca(0xe7e9ec, 0.09),
        danger: c(0xf2777a),
        success: c(0x8bd49c),
        warning: c(0xe6c07b),
        selection: ca(0x9aa4b2, 0.30),
        surface: c(0x26282d),
        user_bubble: c(0x2c2f34),
        tint_blue: c(0x8fb4e8),
        tint_green: c(0x8fcf9f),
        tint_yellow: c(0xdcc27e),
        tint_orange: c(0xdfa07c),
        tint_purple: c(0xbba6e6),
        tint_folder: c(0xaab1ba),
        tint_cyan: c(0x7fcfd6),
        tint_red: c(0xe58a8f),
        tint_pink: c(0xe39ac6),
        dark: true,
    }
}

pub fn light() -> Palette {
    let accent = c(0x6a4fd0);
    Palette {
        bg: c(0xffffff),
        editor: c(0xffffff),
        panel: c(0xf7f7f9),
        bar: c(0xf1f1f4),
        hover: c(0xe8e8ee),
        active: c(0xdcdce4),
        border: c(0xdcdce3),
        fg: c(0x1f1f25),
        muted: c(0x5c5c68),
        accent,
        accent_fill: accent,
        accent_fill_hover: accent.opacity(0.9),
        accent_fill_active: accent.opacity(0.8),
        accent_fg: c(0xffffff),
        accent_soft: ca(0x6a4fd0, 0.12),
        danger: c(0xb3261e),
        success: c(0x1b7a44),
        warning: c(0x8a5a00),
        selection: ca(0x6a4fd0, 0.22),
        surface: c(0xffffff),
        user_bubble: c(0xebe7f8),
        tint_blue: c(0x2f6fbd),
        tint_green: c(0x237a4b),
        tint_yellow: c(0x96700a),
        tint_orange: c(0xb35f22),
        tint_purple: c(0x7a55c7),
        tint_folder: c(0x6d6790),
        tint_cyan: c(0x12808c),
        tint_red: c(0xc23b42),
        tint_pink: c(0xb83a86),
        dark: false,
    }
}

pub fn pal(cx: &App) -> Palette {
    *cx.global::<Palette>()
}

/// Terminal preferences read while painting.
#[derive(Clone, Copy)]
pub struct TermPrefs {
    pub font_size: f32,
}

impl Global for TermPrefs {}

/// Does this choice mean dark right now?
pub fn is_dark(choice: ThemeChoice, cx: &App) -> bool {
    match choice {
        ThemeChoice::Dark => true,
        ThemeChoice::Light => false,
        ThemeChoice::System => matches!(
            cx.window_appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    }
}

/// Apply settings to KERN's own palette and to the component library.
pub fn apply(settings: &Settings, cx: &mut App) {
    let dark_mode = is_dark(settings.theme, cx);
    let (p, mode) = if dark_mode {
        (dark(), ThemeMode::Dark)
    } else {
        (light(), ThemeMode::Light)
    };
    cx.set_global(p);
    crate::terminal::LIGHT_THEME.store(!dark_mode, std::sync::atomic::Ordering::Relaxed);
    cx.set_global(TermPrefs {
        font_size: settings.terminal.font_size,
    });
    crate::terminal::SCROLLBACK.store(
        settings.terminal.scrollback,
        std::sync::atomic::Ordering::Relaxed,
    );
    let editor = settings.editor.clone();
    Theme::change(mode, None, cx);
    Theme::update(cx, |t| {
        t.font_size = px(13.);
        t.mono_font_size = px(editor.font_size);
        t.mono_font_family = editor.font_family.clone().into();
        t.radius = px(4.);
        t.radius_lg = px(6.);
        t.shadow = false;
        t.focus_ring = true;
        t.background = p.bg;
        t.foreground = p.fg;
        t.border = p.border;
        t.input = p.border;
        t.ring = p.accent;
        t.caret = p.accent;
        t.selection = p.selection;
        t.primary = p.accent;
        t.primary_hover = p.accent.opacity(0.9);
        t.primary_active = p.accent.opacity(0.8);
        t.primary_foreground = p.accent_fg;
        t.button_primary = p.accent;
        t.button_primary_hover = p.accent.opacity(0.9);
        t.button_primary_active = p.accent.opacity(0.8);
        t.button_primary_foreground = p.accent_fg;
        t.secondary = p.hover;
        t.secondary_hover = p.active;
        t.secondary_active = p.active;
        t.secondary_foreground = p.fg;
        t.muted = p.panel;
        t.muted_foreground = p.muted;
        t.accent = p.hover;
        t.accent_foreground = p.fg;
        t.popover = p.bar;
        t.popover_foreground = p.fg;
        t.list_hover = p.hover;
        t.list_active = p.accent_soft;
        t.list_active_border = p.accent;
        t.scrollbar = gpui_kit::transparent_black();
        t.link = p.accent;
        t.link_hover = p.accent;
        t.link_active = p.accent;

        let mut hl = (*t.highlight_theme).clone();
        hl.style.editor_background = Some(p.bg);
        hl.style.editor_foreground = Some(p.fg);
        hl.style.editor_active_line = Some(if !editor.active_line {
            p.bg
        } else if p.dark {
            c(0x202226)
        } else {
            c(0xf3f3f7)
        });
        hl.style.editor_line_number = Some(p.muted);
        hl.style.editor_active_line_number = Some(p.fg);
        hl.style.syntax = syntax_colors(p.dark);
        t.highlight_theme = Arc::new(hl);
    });
}

fn syntax_colors(dark: bool) -> gpui_kit::component::highlighter::SyntaxColors {
    let (keyword, string, comment, function, ty, number, tag, attr, prop, punct, fg) = if dark {
        (
            // Comments 4.6:1 on #191B1E: quiet, still readable.
            "#C5B4F0", "#A9DC9D", "#858C96", "#8FB8F6", "#7FDBCA", "#E6C07B", "#F2A0A8", "#E6C07B",
            "#D2D6DB", "#A0A6AE", "#E7E9EC",
        )
    } else {
        (
            "#6B4FC9", "#2E7D4F", "#8E8E99", "#2B62C9", "#0E7C86", "#A66F00", "#B8434A", "#A66F00",
            "#3A3A44", "#5E5E6A", "#25252B",
        )
    };
    let c = |hex: &str| serde_json::json!({"color": hex});
    let value = serde_json::json!({
        "attribute": c(attr), "boolean": c(number), "comment": c(comment), "comment.doc": c(comment),
        "constant": c(number), "constructor": c(ty), "embedded": c(fg),
        "emphasis": {"font_style": "italic"}, "emphasis.strong": {"font_weight": 700},
        "enum": c(ty), "function": c(function), "hint": c(comment), "keyword": c(keyword), "label": c(prop),
        "link_text": c(keyword), "link_uri": c(function), "number": c(number), "operator": c(punct),
        "predictive": c(comment), "preproc": c(keyword), "primary": c(fg), "property": c(prop),
        "punctuation": c(punct), "punctuation.bracket": c(punct), "punctuation.delimiter": c(punct),
        "punctuation.list_marker": c(keyword), "punctuation.special": c(keyword),
        "string": c(string), "string.escape": c(number), "string.regex": c(number), "string.special": c(number),
        "string.special.symbol": c(number), "tag": c(tag), "tag.doctype": c(tag),
        "text.literal": c(string), "title": c(keyword), "type": c(ty),
        "variable": c(fg), "variable.special": c(keyword), "variant": c(ty)
    });
    serde_json::from_value(value).unwrap_or_default()
}

/// Colors for logic that has no `App` at hand. `System` follows the last applied theme.
pub fn status_palette(choice: ThemeChoice) -> Palette {
    let light = match choice {
        ThemeChoice::Dark => false,
        ThemeChoice::Light => true,
        ThemeChoice::System => {
            crate::terminal::LIGHT_THEME.load(std::sync::atomic::Ordering::Relaxed)
        }
    };
    if light { self::light() } else { dark() }
}
