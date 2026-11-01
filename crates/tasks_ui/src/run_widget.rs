use collections::HashSet;
use gpui::{
    App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, Render, Task, WeakEntity,
    Window, px,
};
use project::TaskSourceKind;
use task::ResolvedTask;
use ui::prelude::*;
use util::ResultExt;
use workspace::Workspace;

use crate::task_contexts;

const MAX_TASKS: usize = 12;

/// IntelliJ's run widget popup: the project's tasks with a run button each, plus links to
/// edit them, see all of them and start debugging.
pub struct RunWidgetMenu {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    tasks: Option<Vec<(TaskSourceKind, ResolvedTask)>>,
    _load_tasks: Task<()>,
}

impl EventEmitter<DismissEvent> for RunWidgetMenu {}

impl Focusable for RunWidgetMenu {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl RunWidgetMenu {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let contexts = workspace
            .update(cx, |workspace, cx| task_contexts(workspace, window, cx))
            .log_err();
        let load_tasks = cx.spawn(async move |this, cx| {
            let Some(contexts) = contexts else {
                return;
            };
            let contexts = std::sync::Arc::new(contexts.await);
            let Ok(Some(task_list)) = this.update(cx, |this, cx| {
                let workspace = this.workspace.upgrade()?;
                let inventory = workspace
                    .read(cx)
                    .project()
                    .read(cx)
                    .task_store()
                    .read(cx)
                    .task_inventory()
                    .cloned()?;
                Some(inventory.update(cx, |inventory, cx| {
                    inventory.used_and_current_resolved_tasks(contexts, cx)
                }))
            }) else {
                return;
            };
            let (used, current) = task_list.await;
            this.update(cx, |this, cx| {
                let mut seen_labels = HashSet::default();
                let tasks = used
                    .into_iter()
                    .chain(current)
                    .filter(|(_, task)| seen_labels.insert(task.resolved_label.clone()))
                    .take(MAX_TASKS)
                    .collect();
                this.tasks = Some(tasks);
                cx.notify();
            })
            .log_err();
        });
        Self {
            workspace,
            focus_handle: cx.focus_handle(),
            tasks: None,
            _load_tasks: load_tasks,
        }
    }

    fn run(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((kind, task)) = self
            .tasks
            .as_ref()
            .and_then(|tasks| tasks.get(index))
            .cloned()
        else {
            return;
        };
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.schedule_resolved_task(kind, task, false, window, cx)
            })
            .log_err();
        cx.emit(DismissEvent);
    }

    fn render_task(
        &self,
        index: usize,
        task: &ResolvedTask,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let radius = ui::islands_control_radius(cx);
        let hover_background = cx.theme().colors().ghost_element_hover;
        let group = SharedString::from(format!("run-widget-task-{index}"));
        h_flex()
            .id(("run-widget-task", index))
            .group(group.clone())
            .h_7()
            .px_2()
            .gap_2()
            .rounded(radius)
            .cursor_pointer()
            .hover(move |style| style.bg(hover_background))
            .child(
                Icon::new(IconName::Terminal)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Label::new(task.resolved_label.clone()).truncate()),
            )
            .child(
                div().visible_on_hover(group).child(
                    Icon::new(IconName::IslandsRun)
                        .size(IconSize::Small)
                        .color(Color::Success),
                ),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.run(index, window, cx)))
    }

    fn render_link(
        &self,
        id: &'static str,
        label: &'static str,
        action: Box<dyn gpui::Action>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let radius = ui::islands_control_radius(cx);
        let hover_background = cx.theme().colors().ghost_element_hover;
        h_flex()
            .id(id)
            .h_7()
            .px_2()
            .rounded(radius)
            .cursor_pointer()
            .hover(move |style| style.bg(hover_background))
            .child(Label::new(label))
            .on_click(cx.listener(move |_, _, window, cx| {
                cx.emit(DismissEvent);
                window.dispatch_action(action.boxed_clone(), cx);
            }))
    }
}

impl Render for RunWidgetMenu {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let border_color = colors.border_variant;
        let background = colors.elevated_surface_background;
        let outline = colors.border;
        let task_rows = match &self.tasks {
            None => vec![
                div()
                    .px_2()
                    .py_1()
                    .child(Label::new("Loading tasks…").color(Color::Muted))
                    .into_any_element(),
            ],
            Some(tasks) if tasks.is_empty() => vec![
                div()
                    .px_2()
                    .py_1()
                    .child(Label::new("No tasks in this project").color(Color::Muted))
                    .into_any_element(),
            ],
            Some(tasks) => tasks
                .iter()
                .enumerate()
                .map(|(index, (_, task))| self.render_task(index, task, cx).into_any_element())
                .collect(),
        };
        // The debugger's actions live in a crate tasks_ui doesn't depend on.
        let debug_action = cx.build_action("debugger::Start", None).log_err();

        v_flex()
            .key_context("RunWidgetMenu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .w(px(340.))
            .p_1()
            .gap_px()
            .bg(background)
            .border_1()
            .border_color(outline)
            .rounded(ui::islands_style(cx).corner_radius)
            .shadow_lg()
            .children(task_rows)
            .child(div().my_1().h_px().w_full().bg(border_color))
            .child(self.render_link(
                "run-widget-all-tasks",
                "All Tasks…",
                Box::new(zed_actions::Spawn::modal()),
                cx,
            ))
            .when_some(debug_action, |this, debug_action| {
                this.child(self.render_link("run-widget-debug", "Debug…", debug_action, cx))
            })
            .child(self.render_link(
                "run-widget-edit",
                "Edit Configurations…",
                Box::new(zed_actions::OpenProjectTasks),
                cx,
            ))
    }
}
