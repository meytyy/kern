//! KERN's design vocabulary: one set of buttons, headers, rows, cards and empty
//! states, so that every panel looks and behaves the same. Icon-only controls
//! always carry a tooltip and an accessible name, and take keyboard focus.
use super::assets::icon;
use super::theme::Palette;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::*;

pub const ICON_SM: f32 = 12.;
pub const ICON_MD: f32 = 14.;
pub const ICON_LG: f32 = 16.;
pub const ICON_FILE: f32 = 16.;
pub const HEADER_H: f32 = 32.;
pub const ROW_H: f32 = 24.;
pub const RADIUS: f32 = 4.;
pub const RADIUS_LG: f32 = 7.;

pub fn icon_el(name: &'static str, size: f32, color: Hsla) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .text_color(color)
        .child(icon(name).with_size(px(size)).text_color(color))
}

/// For a text area inside a form or chat: Tab moves on instead of typing spaces.
pub fn tab_moves_focus(el: Div) -> Div {
    use gpui_kit::component::input::{IndentInline, OutdentInline};
    el.capture_action(|_: &IndentInline, window, cx| {
        window.focus_next(cx);
        cx.stop_propagation();
    })
    .capture_action(|_: &OutdentInline, window, cx| {
        window.focus_prev(cx);
        cx.stop_propagation();
    })
}

/// A flat secondary action.
pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, _p: Palette) -> Button {
    Button::new(id)
        .secondary()
        .small()
        .compact()
        .label(label)
        .h(px(ROW_H))
        .flex_shrink_0()
}

/// The main action of a surface. Use one per surface.
pub fn primary_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    _p: Palette,
) -> Button {
    Button::new(id)
        .primary()
        .small()
        .compact()
        .label(label)
        .h(px(ROW_H))
        .flex_shrink_0()
}

/// An action that sits in a list or toolbar and should not draw attention.
pub fn ghost_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    _p: Palette,
) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .compact()
        .label(label)
        .h(px(ROW_H))
        .flex_shrink_0()
}

/// Icon-only control: tooltip, accessible name and focus come with it.
pub fn icon_button(
    id: impl Into<ElementId>,
    name: &'static str,
    label: impl Into<SharedString>,
) -> Button {
    let label: SharedString = label.into();
    Button::new(id)
        .ghost()
        .icon(icon(name).with_size(px(ICON_MD)))
        .tooltip(label.clone())
        .accessibility_label(label)
        .size(px(ROW_H))
        .flex_shrink_0()
}

/// Same, for a toggle that shows whether it is on.
pub fn icon_toggle(
    id: impl Into<ElementId>,
    name: &'static str,
    label: impl Into<SharedString>,
    on: bool,
    p: Palette,
) -> Button {
    let label: SharedString = label.into();
    let color = if on { p.accent } else { p.muted };
    Button::new(id)
        .ghost()
        .child(icon_el(name, ICON_MD, color))
        .tooltip(label.clone())
        .accessibility_label(label)
        .size(px(ROW_H))
        .flex_shrink_0()
        .when(on, |b| b.bg(p.accent_soft))
}

/// The title row every side panel starts with; actions go in as children.
pub fn panel_header(title: impl Into<SharedString>, p: Palette) -> Div {
    let t: SharedString = title.into();
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(2.))
        .h(px(HEADER_H))
        .pl_3()
        .pr_1()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.fg)
                .child(t),
        )
}

/// A secondary group label inside a panel.
pub fn section_title(text: impl Into<SharedString>, p: Palette) -> Div {
    let t: SharedString = text.into();
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .h(px(ROW_H))
        .px_3()
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.muted)
        .child(t)
}

/// A selectable row in a list.
pub fn list_row(id: impl Into<ElementId>, selected: bool, p: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_1()
        .min_h(px(ROW_H))
        .px_2()
        .rounded(px(RADIUS))
        .when(selected, |d| d.bg(p.accent_soft))
        .hover(move |s| s.bg(if selected { p.accent_soft } else { p.hover }))
        .cursor_pointer()
}

/// A raised surface for one item.
pub fn card(p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_2()
        .rounded(px(RADIUS_LG))
        .bg(p.surface)
        .border_1()
        .border_color(p.border)
}

/// A small status label.
pub fn chip(label: impl Into<SharedString>, color: Hsla) -> Div {
    let label: SharedString = label.into();
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .text_size(px(11.))
        .text_color(color)
        .child(div().size(px(6.)).rounded_full().bg(color))
        .child(label)
}

/// What a panel shows when it has nothing to show.
pub fn empty_state(
    icon_name: &'static str,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    p: Palette,
) -> Div {
    let (title, detail): (SharedString, SharedString) = (title.into(), detail.into());
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_1()
        .p_4()
        .text_center()
        .child(icon_el(icon_name, 22., p.muted))
        .child(
            div()
                .mt_1()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .child(title),
        )
        .child(
            div()
                .w_full()
                .text_size(px(12.))
                .text_color(p.muted)
                .child(detail),
        )
}

/// A monospaced block for commands and paths, wrapping instead of overflowing.
pub fn code_text(text: impl Into<SharedString>, p: Palette) -> Div {
    let text: SharedString = text.into();
    div()
        .px_2()
        .py_1()
        .rounded(px(RADIUS))
        .bg(p.editor)
        .border_1()
        .border_color(p.border)
        .font_family(crate::core::settings::MONO_FONT)
        .text_size(px(11.))
        .child(text)
}

pub fn hsep(p: Palette) -> Div {
    div().h(px(1.)).w_full().flex_shrink_0().bg(p.border)
}

pub fn vsep(p: Palette) -> Div {
    div().w(px(1.)).h_full().flex_shrink_0().bg(p.border)
}

/// A file or folder icon from the registry, in the common 16 pt column.
pub fn registry_icon(
    found: super::file_icons::FileIcon,
    dim: bool,
    p: Palette,
) -> impl IntoElement {
    let color = found.tint.color(p);
    icon_el(
        found.svg,
        ICON_FILE,
        if dim { color.opacity(0.55) } else { color },
    )
}

pub fn logo_image(size: f32) -> impl IntoElement {
    use std::sync::{Arc, OnceLock};
    static LOGO: OnceLock<Arc<Image>> = OnceLock::new();
    let image = LOGO
        .get_or_init(|| {
            Arc::new(Image::from_bytes(
                ImageFormat::Png,
                super::assets::LOGO_PNG.to_vec(),
            ))
        })
        .clone();
    img(image).size(px(size)).flex_shrink_0()
}

/// A tooltip for any element: `.tooltip(tip("Run (⌘R)"))`. It shows on hover,
/// stays inside the window and never takes clicks or the focus.
pub fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx)
}
