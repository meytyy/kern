//! Short GPUI stack probe: window, editable text, Cyrillic input, scrolling, system folder dialog.
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::*;

struct Probe {
    editor: Entity<EditorState>,
    picked: SharedString,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .p_2()
                    .child(
                        Button::new("open")
                            .label("Open Folder")
                            .on_click(cx.listener(|_, _, _, cx| {
                                let rx = cx.prompt_for_paths(PathPromptOptions {
                                    files: false,
                                    directories: true,
                                    multiple: false,
                                    prompt: None,
                                });
                                cx.spawn(async move |this, cx| {
                                    if let Ok(Ok(Some(paths))) = rx.await {
                                        this.update(cx, |this, cx| {
                                            this.picked = format!("{:?}", paths[0]).into();
                                            cx.notify();
                                        })
                                        .ok();
                                    }
                                })
                                .detach();
                            })),
                    )
                    .child(self.picked.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Editor::new(&self.editor).h_full()),
            )
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        let bounds = Bounds::centered(None, size(px(900.), px(600.)), cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let mut text = String::from(
                    "// Кириллица: Съешь ещё этих мягких французских булок\nconst App = () => <div className=\"x\">Привет</div>;\n",
                );
                for i in 0..2000 {
                    text.push_str(&format!("const line{i} = {i}; // строка {i}\n"));
                }
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .language("tsx")
                        .line_number(true)
                        .default_value(text)
                });
                editor.update(cx, |e, cx| e.focus(window, cx));
                cx.activate(true);
                cx.new(|_| Probe { editor, picked: "no folder".into() })
            },
        )
        .expect("open window");
    });
}
