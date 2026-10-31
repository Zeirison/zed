use std::{collections::BTreeMap, ops::Range, sync::Arc, time::Duration};

use collections::{HashSet, IndexSet};
use editor::Editor;
use file_icons::FileIcons;
use gpui::{
    Action, App, AsyncWindowContext, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, KeyContext, ListSizingBehavior, Pixels, Render, ScrollStrategy,
    Subscription, Task, UniformListScrollHandle, WeakEntity, Window, actions, uniform_list,
};
use language::Point;
use project::{Project, ProjectPath};
use settings::Settings;
use ui::{Tooltip, prelude::*};
use util::ResultExt;
use workspace::{
    Workspace, WorkspaceSettings,
    dock::{DockPosition, Panel, PanelEvent, panel_header_height, render_panel_header_controls_at},
};

use crate::Deploy;

const PROBLEMS_PANEL_KEY: &str = "ProblemsPanel";
const UPDATE_DEBOUNCE: Duration = Duration::from_millis(50);

actions!(
    problems_panel,
    [
        /// Toggles focus on the Problems tool window. Outside the islands layout
        /// it opens the project diagnostics view instead.
        ToggleFocus
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(
        |workspace: &mut Workspace, _window, _: &mut Context<Workspace>| {
            workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
                if WorkspaceSettings::get_global(cx).islands.enabled {
                    workspace.toggle_panel_focus::<ProblemsPanel>(window, cx);
                } else {
                    window.dispatch_action(Deploy.boxed_clone(), cx);
                }
            });
        },
    )
    .detach();
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProblemsTab {
    CurrentFile,
    Project,
}

struct Problem {
    severity: lsp::DiagnosticSeverity,
    message: SharedString,
    source: Option<SharedString>,
    position: Point,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProblemRow {
    File(usize),
    Problem(usize, usize),
}

/// IntelliJ's Problems tool window: the errors and warnings of the project,
/// grouped by file, in the bottom dock of the islands layout.
pub struct ProblemsPanel {
    project: Entity<Project>,
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    position: DockPosition,
    active_tab: ProblemsTab,
    problems: BTreeMap<ProjectPath, Vec<Problem>>,
    collapsed: HashSet<ProjectPath>,
    current_file: Option<ProjectPath>,
    selected: Option<ProblemRow>,
    paths_to_update: IndexSet<ProjectPath>,
    update_task: Option<Task<anyhow::Result<()>>>,
    _subscriptions: Vec<Subscription>,
}

impl ProblemsPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> anyhow::Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            let workspace_entity = cx.entity();
            let project = workspace.project().clone();
            let current_file = workspace
                .active_item(cx)
                .and_then(|item| item.project_path(cx));
            cx.new(|cx| ProblemsPanel::new(workspace_entity, project, current_file, window, cx))
        })
    }

    fn new(
        workspace: Entity<Workspace>,
        project: Entity<Project>,
        current_file: Option<ProjectPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.subscribe_in(&project, window, |this, _, event, window, cx| {
                if let project::Event::DiagnosticsUpdated { paths, .. } = event {
                    this.paths_to_update.extend(paths.iter().cloned());
                    this.schedule_update(window, cx);
                }
            }),
            cx.subscribe(&workspace, |this, workspace, event, cx| {
                if let workspace::Event::ActiveItemChanged = event {
                    this.current_file = workspace
                        .read(cx)
                        .active_item(cx)
                        .and_then(|item| item.project_path(cx));
                    cx.notify();
                }
            }),
        ];
        let mut this = Self {
            current_file,
            project,
            workspace: workspace.downgrade(),
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            position: DockPosition::Bottom,
            active_tab: ProblemsTab::Project,
            problems: BTreeMap::new(),
            collapsed: HashSet::default(),
            selected: None,
            paths_to_update: IndexSet::default(),
            update_task: None,
            _subscriptions: subscriptions,
        };
        let paths = this
            .project
            .read(cx)
            .diagnostic_summaries(false, cx)
            .map(|(path, _, _)| path)
            .collect::<Vec<_>>();
        this.paths_to_update.extend(paths);
        this.schedule_update(window, cx);
        this
    }

    fn schedule_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.update_task.is_some() {
            return;
        }
        let project = self.project.clone();
        self.update_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(UPDATE_DEBOUNCE).await;
            loop {
                let Some(path) = this.update(cx, |this, cx| {
                    let path = this.paths_to_update.pop();
                    if path.is_none() {
                        this.update_task = None;
                        cx.notify();
                    }
                    path
                })?
                else {
                    break;
                };

                let summary = project.read_with(cx, |project, cx| {
                    project.diagnostic_summary_for_path(&path, cx)
                });
                if summary.error_count + summary.warning_count == 0 {
                    this.update(cx, |this, _| this.problems.remove(&path))?;
                    continue;
                }

                let Some(buffer) = project
                    .update(cx, |project, cx| project.open_buffer(path.clone(), cx))
                    .await
                    .log_err()
                else {
                    continue;
                };
                let snapshot = buffer.read_with(cx, |buffer, _| buffer.snapshot());
                let mut problems = snapshot
                    .diagnostics_in_range::<_, Point>(Point::zero()..snapshot.max_point(), false)
                    .filter(|entry| {
                        entry.diagnostic.is_primary
                            && entry.diagnostic.severity <= lsp::DiagnosticSeverity::WARNING
                    })
                    .map(|entry| Problem {
                        severity: entry.diagnostic.severity,
                        message: entry
                            .diagnostic
                            .message
                            .as_str()
                            .lines()
                            .next()
                            .unwrap_or_default()
                            .to_string()
                            .into(),
                        source: entry.diagnostic.source.clone().map(SharedString::from),
                        position: entry.range.start,
                    })
                    .collect::<Vec<_>>();
                problems.sort_by_key(|problem| (problem.severity, problem.position));
                this.update(cx, |this, _| {
                    if problems.is_empty() {
                        this.problems.remove(&path);
                    } else {
                        this.problems.insert(path, problems);
                    }
                })?;
            }
            Ok(())
        }));
    }

    fn visible_files(&self) -> Vec<(&ProjectPath, &Vec<Problem>)> {
        match self.active_tab {
            ProblemsTab::Project => self.problems.iter().collect(),
            ProblemsTab::CurrentFile => self
                .current_file
                .as_ref()
                .and_then(|path| self.problems.get_key_value(path))
                .into_iter()
                .collect(),
        }
    }

    fn rows(&self) -> Vec<ProblemRow> {
        let mut rows = Vec::new();
        for (file_index, (path, problems)) in self.visible_files().into_iter().enumerate() {
            rows.push(ProblemRow::File(file_index));
            if !self.collapsed.contains(path) {
                rows.extend(
                    (0..problems.len()).map(|index| ProblemRow::Problem(file_index, index)),
                );
            }
        }
        rows
    }

    fn problem_count(&self, tab: ProblemsTab) -> usize {
        match tab {
            ProblemsTab::Project => self.problems.values().map(Vec::len).sum(),
            ProblemsTab::CurrentFile => self
                .current_file
                .as_ref()
                .and_then(|path| self.problems.get(path))
                .map_or(0, Vec::len),
        }
    }

    fn set_active_tab(&mut self, tab: ProblemsTab, cx: &mut Context<Self>) {
        self.active_tab = tab;
        self.selected = None;
        cx.notify();
    }

    fn select(&mut self, row: ProblemRow, cx: &mut Context<Self>) {
        self.selected = Some(row);
        if let Some(index) = self.rows().iter().position(|candidate| *candidate == row) {
            self.scroll_handle
                .scroll_to_item(index, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn select_next(&mut self, _: &menu::SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    fn select_previous(
        &mut self,
        _: &menu::SelectPrevious,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selection(-1, cx);
    }

    fn move_selection(&mut self, offset: isize, cx: &mut Context<Self>) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let next_index = match self
            .selected
            .and_then(|selected| rows.iter().position(|row| *row == selected))
        {
            Some(index) => index
                .saturating_add_signed(offset)
                .min(rows.len().saturating_sub(1)),
            None => 0,
        };
        if let Some(row) = rows.get(next_index).copied() {
            self.select(row, cx);
        }
    }

    fn collapse_selected(
        &mut self,
        _: &menu::SelectParent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let file_index = match self.selected {
            Some(ProblemRow::File(file_index)) | Some(ProblemRow::Problem(file_index, _)) => {
                file_index
            }
            None => return,
        };
        if let Some(path) = self.path_at(file_index) {
            self.collapsed.insert(path);
        }
        self.select(ProblemRow::File(file_index), cx);
    }

    fn expand_selected(&mut self, _: &menu::SelectChild, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ProblemRow::File(file_index)) = self.selected
            && let Some(path) = self.path_at(file_index)
        {
            self.collapsed.remove(&path);
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected {
            Some(ProblemRow::Problem(file_index, index)) => {
                self.open_problem(file_index, index, true, window, cx)
            }
            Some(ProblemRow::File(file_index)) => self.toggle_collapsed(file_index, cx),
            None => {}
        }
    }

    fn path_at(&self, file_index: usize) -> Option<ProjectPath> {
        self.visible_files()
            .get(file_index)
            .map(|(path, _)| (*path).clone())
    }

    fn toggle_collapsed(&mut self, file_index: usize, cx: &mut Context<Self>) {
        if let Some(path) = self.path_at(file_index)
            && !self.collapsed.remove(&path)
        {
            self.collapsed.insert(path);
        }
        cx.notify();
    }

    fn open_problem(
        &mut self,
        file_index: usize,
        index: usize,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((path, position)) =
            self.visible_files()
                .get(file_index)
                .and_then(|(path, problems)| {
                    problems
                        .get(index)
                        .map(|problem| ((*path).clone(), problem.position))
                })
        else {
            return;
        };
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let open_task = workspace.update(cx, |workspace, cx| {
            workspace.open_path(path, None, focus, window, cx)
        });
        cx.spawn_in(window, async move |_, cx| {
            let item = open_task.await?;
            if let Some(editor) = item.downcast::<Editor>() {
                editor.update_in(cx, |editor, window, cx| {
                    editor.go_to_singleton_buffer_point(position, window, cx);
                })?;
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn render_header(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let radius = ui::islands_control_radius(cx);
        let tab =
            |id: &'static str, label: &'static str, tab: ProblemsTab, cx: &mut Context<Self>| {
                let active = self.active_tab == tab;
                let active_background = cx.theme().colors().ghost_element_active;
                let hover_background = cx.theme().colors().ghost_element_hover;
                let count = self.problem_count(tab);
                h_flex()
                    .id(id)
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded(radius)
                    .cursor_pointer()
                    .map(|this| {
                        if active {
                            this.bg(active_background)
                        } else {
                            this.hover(move |style| style.bg(hover_background))
                        }
                    })
                    .child(Label::new(label))
                    .when(count > 0, |this| {
                        this.child(Label::new(count.to_string()).color(Color::Muted))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.set_active_tab(tab, cx)))
            };
        let current_file_tab = tab(
            "problems-current-file-tab",
            "Current File",
            ProblemsTab::CurrentFile,
            cx,
        );
        let project_tab = tab("problems-project-tab", "Project", ProblemsTab::Project, cx);

        h_flex()
            .id("problems-panel-header")
            .flex_none()
            .h(panel_header_height(cx))
            .w_full()
            .pl_3()
            .pr_1()
            .gap_0p5()
            .child(Label::new("Problems").weight(FontWeight::SEMIBOLD).mr_2())
            .child(current_file_tab)
            .child(project_tab)
            .child(div().flex_1())
            .child(render_panel_header_controls_at(
                Arc::new(cx.entity()),
                self.position(window, cx),
                "Problems".into(),
            ))
    }

    fn render_row(&self, row: ProblemRow, row_index: usize, cx: &mut Context<Self>) -> AnyElement {
        let files = self.visible_files();
        let selected = self.selected == Some(row);
        let selected_background = cx.theme().colors().ghost_element_selected;
        let hover_background = cx.theme().colors().ghost_element_hover;
        let radius = ui::islands_control_radius(cx);
        let path_style = self.project.read(cx).path_style(cx);
        let base = h_flex()
            .id(("problem-row", row_index))
            .h(px(24.))
            .w_full()
            .gap_1p5()
            .pr_2()
            .rounded(radius)
            .cursor_pointer()
            .map(|this| {
                if selected {
                    this.bg(selected_background)
                } else {
                    this.hover(move |style| style.bg(hover_background))
                }
            });

        match row {
            ProblemRow::File(file_index) => {
                let Some((path, problems)) = files.get(file_index) else {
                    return div().into_any_element();
                };
                let collapsed = self.collapsed.contains(*path);
                let file_name = path
                    .path
                    .file_name()
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| path.path.display(path_style).to_string());
                let directory = path
                    .path
                    .parent()
                    .filter(|parent| !parent.is_empty())
                    .map(|parent| parent.display(path_style).to_string());
                let file_icon = FileIcons::get_icon(path.path.as_std_path(), cx);
                let count = problems.len();
                let count_label = if count == 1 {
                    "1 problem".to_string()
                } else {
                    format!("{count} problems")
                };
                base.pl_1()
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .size(IconSize::Small)
                        .color(Color::Muted),
                    )
                    .when_some(file_icon, |this, icon| {
                        this.child(Icon::from_path(icon).color(Color::Muted))
                    })
                    .child(Label::new(file_name))
                    .when_some(directory, |this, directory| {
                        this.child(Label::new(directory).color(Color::Muted).truncate())
                    })
                    .child(Label::new(count_label).color(Color::Muted))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(row);
                        this.toggle_collapsed(file_index, cx);
                    }))
                    .into_any_element()
            }
            ProblemRow::Problem(file_index, index) => {
                let Some(problem) = files
                    .get(file_index)
                    .and_then(|(_, problems)| problems.get(index))
                else {
                    return div().into_any_element();
                };
                let (icon, color) = if problem.severity == lsp::DiagnosticSeverity::ERROR {
                    (IconName::XCircle, Color::Error)
                } else {
                    (IconName::Warning, Color::Warning)
                };
                let location = format!(
                    ":{}:{}",
                    problem.position.row + 1,
                    problem.position.column + 1
                );
                base.pl(px(28.))
                    .child(Icon::new(icon).size(IconSize::Small).color(color))
                    .child(Label::new(problem.message.clone()).truncate())
                    .child(Label::new(location).color(Color::Muted).flex_none())
                    .when_some(problem.source.clone(), |this, source| {
                        this.child(Label::new(source).color(Color::Muted).flex_none())
                    })
                    .tooltip(Tooltip::text(problem.message.clone()))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        this.selected = Some(row);
                        let focus_editor = event.click_count() > 1;
                        this.open_problem(file_index, index, focus_editor, window, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            }
        }
    }

    fn render_empty_state(&self) -> impl IntoElement {
        let message: SharedString = match self.active_tab {
            ProblemsTab::Project => "No problems found".into(),
            ProblemsTab::CurrentFile => match &self.current_file {
                Some(path) => format!(
                    "No problems in {}",
                    path.path.file_name().unwrap_or_default()
                )
                .into(),
                None => "No file is open".into(),
            },
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .child(Label::new(message).color(Color::Muted))
    }

    fn dispatch_context(&self) -> KeyContext {
        let mut context = KeyContext::default();
        context.add("ProblemsPanel");
        context.add("menu");
        context
    }
}

impl EventEmitter<PanelEvent> for ProblemsPanel {}

impl Focusable for ProblemsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ProblemsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows();
        let row_count = rows.len();
        v_flex()
            .size_full()
            .key_context(self.dispatch_context())
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::collapse_selected))
            .on_action(cx.listener(Self::expand_selected))
            .on_action(cx.listener(Self::confirm))
            .child(self.render_header(window, cx))
            .child(if row_count == 0 {
                self.render_empty_state().into_any_element()
            } else {
                uniform_list(
                    "problems",
                    row_count,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        let rows = this.rows();
                        range
                            .filter_map(|row_index| {
                                rows.get(row_index)
                                    .map(|row| this.render_row(*row, row_index, cx))
                            })
                            .collect()
                    }),
                )
                .px_1()
                .flex_1()
                .size_full()
                .with_sizing_behavior(ListSizingBehavior::Infer)
                .track_scroll(&self.scroll_handle)
                .into_any_element()
            })
    }
}

impl Panel for ProblemsPanel {
    fn persistent_name() -> &'static str {
        "ProblemsPanel"
    }

    fn panel_key() -> &'static str {
        PROBLEMS_PANEL_KEY
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
            DockPosition::Bottom => px(260.),
        }
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::IslandsProblems)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Problems")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        9
    }

    fn enabled(&self, cx: &App) -> bool {
        WorkspaceSettings::get_global(cx).islands.enabled
    }

    fn draws_own_islands_header(&self) -> bool {
        true
    }
}
