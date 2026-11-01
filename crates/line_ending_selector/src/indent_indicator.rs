use editor::Editor;
use gpui::{App, Entity, Subscription};
use ui::{Tooltip, prelude::*};
use workspace::{HideStatusItem, StatusItemView, item::ItemHandle};

/// IntelliJ's indent widget ("4 spaces" or "Tab"), shown in the islands status bar.
#[derive(Default)]
pub struct IndentIndicator {
    label: Option<SharedString>,
    _observe_active_editor: Option<Subscription>,
}

impl IndentIndicator {
    fn update(&mut self, editor: Entity<Editor>, _: &mut Window, cx: &mut Context<Self>) {
        let editor = editor.read(cx);
        self.label = editor.active_buffer(cx).map(|_| {
            let settings = editor.buffer().read(cx).language_settings(cx);
            if settings.hard_tabs {
                SharedString::from("Tab")
            } else {
                format!("{} spaces", settings.tab_size).into()
            }
        });
        cx.notify();
    }
}

impl Render for IndentIndicator {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !ui::islands_style(cx).enabled {
            return div();
        }
        div().when_some(self.label.clone(), |this, label| {
            let tooltip: SharedString = format!("Indent: {label}").into();
            this.child(
                Button::new("indent-indicator", label)
                    .label_size(LabelSize::Small)
                    .tooltip(Tooltip::text(tooltip)),
            )
        })
    }
}

impl StatusItemView for IndentIndicator {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = active_pane_item.and_then(|item| item.downcast::<Editor>()) {
            self._observe_active_editor = Some(cx.observe_in(&editor, window, Self::update));
            self.update(editor, window, cx);
        } else {
            self.label = None;
            self._observe_active_editor = None;
        }
        cx.notify();
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}
