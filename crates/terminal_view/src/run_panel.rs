use std::sync::Arc;

use gpui::{
    Action, App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, ParentElement,
    Pixels, Render, Styled, Subscription, WeakEntity, Window, actions, px,
};
use settings::Settings;
use ui::{Tooltip, prelude::*};
use workspace::{
    Pane, Workspace, WorkspaceSettings,
    dock::{DockPosition, Panel, PanelEvent, render_panel_header_controls, render_panel_header_title},
    pane,
};

use crate::{RerunTask, TerminalView};

const RUN_PANEL_KEY: &str = "RunPanel";

actions!(
    run_panel,
    [
        /// Toggles focus on the Run tool window, where tasks show their output.
        ToggleFocus,
        /// Toggles the Run tool window.
        Toggle
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(
        |workspace: &mut Workspace, _window, _: &mut Context<Workspace>| {
            workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
                workspace.toggle_panel_focus::<RunPanel>(window, cx);
            });
            workspace.register_action(|workspace, _: &Toggle, window, cx| {
                if !workspace.toggle_panel_focus::<RunPanel>(window, cx) {
                    workspace.close_panel::<RunPanel>(window, cx);
                }
            });
        },
    )
    .detach();
}

/// IntelliJ's Run tool window: the terminals of tasks spawned in the dock live
/// here instead of next to the user's shells when the islands layout is on.
pub struct RunPanel {
    pane: Entity<Pane>,
    position: DockPosition,
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    _subscriptions: Vec<Subscription>,
}

impl RunPanel {
    pub fn new(workspace: &Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let weak_workspace = workspace.weak_handle();
        let project = workspace.project().clone();
        let pane = cx.new(|cx| {
            let mut pane = Pane::new(
                weak_workspace.clone(),
                project,
                Default::default(),
                None,
                workspace::NewTerminal::default().boxed_clone(),
                false,
                window,
                cx,
            );
            pane.set_can_navigate(false, cx);
            pane.display_nav_history_buttons(None);
            pane.set_should_display_tab_bar(|_, _| true);
            pane.set_zoom_out_on_close(false);
            pane.set_can_split(None);
            pane
        });
        let run_panel = cx.entity().downgrade();
        pane.update(cx, |pane, cx| {
            pane.set_render_tab_bar_buttons(cx, move |pane, window, cx| {
                let Some(run_panel) = run_panel.upgrade() else {
                    return (None, None);
                };
                let title = div()
                    .pl_1()
                    .pr_2()
                    .child(render_panel_header_title(&run_panel, window, cx))
                    .into_any_element();
                let active_terminal = pane
                    .active_item()
                    .and_then(|item| item.downcast::<TerminalView>());
                let controls = h_flex()
                    .gap_0p5()
                    .when_some(active_terminal, |this, terminal_view| {
                        this.child(
                            IconButton::new("run-panel-rerun", IconName::RotateCw)
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text("Rerun"))
                                .on_click({
                                    let terminal_view = terminal_view.downgrade();
                                    move |_, window, cx| {
                                        terminal_view
                                            .update(cx, |terminal_view, cx| {
                                                terminal_view.rerun_task(&RerunTask, window, cx)
                                            })
                                            .ok();
                                    }
                                }),
                        )
                        .child(
                            IconButton::new("run-panel-stop", IconName::Stop)
                                .icon_size(IconSize::Small)
                                .icon_color(Color::Error)
                                .tooltip(Tooltip::text("Stop"))
                                .on_click(move |_, _, cx| {
                                    terminal_view.update(cx, |terminal_view, cx| {
                                        terminal_view.terminal().update(cx, |terminal, _| {
                                            terminal.kill_active_task()
                                        })
                                    })
                                }),
                        )
                    })
                    .child(render_panel_header_controls(Arc::new(run_panel), window, cx))
                    .into_any_element();
                (Some(title), Some(controls))
            });
        });
        let subscriptions = vec![cx.subscribe_in(&pane, window, Self::handle_pane_event)];
        Self {
            pane,
            position: DockPosition::Bottom,
            focus_handle: cx.focus_handle(),
            workspace: weak_workspace,
            _subscriptions: subscriptions,
        }
    }

    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> anyhow::Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            cx.new(|cx| RunPanel::new(workspace, window, cx))
        })
    }

    fn handle_pane_event(
        &mut self,
        pane: &Entity<Pane>,
        event: &pane::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            pane::Event::AddItem { item } => {
                if let Some(workspace) = self.workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        item.added_to_pane(workspace, pane.clone(), window, cx)
                    })
                }
            }
            pane::Event::Remove { .. } => cx.emit(PanelEvent::Close),
            pane::Event::ZoomIn => {
                pane.update(cx, |pane, cx| pane.set_zoomed(true, cx));
                cx.emit(PanelEvent::ZoomIn);
            }
            pane::Event::ZoomOut => {
                pane.update(cx, |pane, cx| pane.set_zoomed(false, cx));
                cx.emit(PanelEvent::ZoomOut);
            }
            pane::Event::Focus => cx.emit(PanelEvent::Activate),
            _ => {}
        }
    }
}

/// The Run tool window's pane when the islands layout routes task terminals there.
pub(crate) fn run_pane(workspace: &Workspace, cx: &App) -> Option<Entity<Pane>> {
    if !WorkspaceSettings::get_global(cx).islands.enabled {
        return None;
    }
    let run_panel = workspace.panel::<RunPanel>(cx)?;
    Some(run_panel.read(cx).pane.clone())
}

impl EventEmitter<PanelEvent> for RunPanel {}

impl Focusable for RunPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RunPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_empty = self.pane.read(cx).items_len() == 0;
        div()
            .relative()
            .size_full()
            .track_focus(&self.focus_handle)
            .child(self.pane.clone())
            .when(is_empty, |this| {
                this.child(
                    v_flex()
                        .absolute()
                        .inset_0()
                        .top_10()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .child(Label::new("Nothing is running").color(Color::Muted))
                        .child(
                            Button::new("run-panel-run-task", "Run Task…")
                                .style(ButtonStyle::Filled)
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(
                                        zed_actions::Spawn::modal().boxed_clone(),
                                        cx,
                                    )
                                }),
                        ),
                )
            })
    }
}

impl Panel for RunPanel {
    fn persistent_name() -> &'static str {
        "RunPanel"
    }

    fn panel_key() -> &'static str {
        RUN_PANEL_KEY
    }

    fn activation_focus_handle(&self, cx: &App) -> FocusHandle {
        self.pane.focus_handle(cx)
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, _position: DockPosition) -> bool {
        true
    }

    fn set_position(
        &mut self,
        position: DockPosition,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.position = position;
        cx.notify();
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        match self.position {
            DockPosition::Left | DockPosition::Right => px(400.),
            DockPosition::Bottom => px(300.),
        }
    }

    fn is_zoomed(&self, _window: &Window, cx: &App) -> bool {
        self.pane.read(cx).is_zoomed()
    }

    fn set_zoomed(&mut self, zoomed: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.pane.update(cx, |pane, cx| pane.set_zoomed(zoomed, cx));
        cx.notify();
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::IslandsRun)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Run")
    }

    fn icon_label(&self, _window: &Window, cx: &App) -> Option<String> {
        let count = self.pane.read(cx).items_len();
        (count > 1).then(|| count.to_string())
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn pane(&self) -> Option<Entity<Pane>> {
        Some(self.pane.clone())
    }

    fn activation_priority(&self) -> u32 {
        8
    }

    fn enabled(&self, cx: &App) -> bool {
        WorkspaceSettings::get_global(cx).islands.enabled
    }

    fn draws_own_islands_header(&self) -> bool {
        true
    }
}
