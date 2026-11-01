use std::sync::Arc;

use collections::HashSet;
use git::repository::Branch;
use gpui::{
    Action, App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, Pixels, Render, SharedString, Subscription, WeakEntity, Window, actions,
};
use project::git_store::{GitStore, GitStoreEvent, Repository, RepositoryId};
use settings::Settings;
use ui::prelude::*;
use workspace::{
    Workspace, WorkspaceSettings,
    dock::{DockPosition, Panel, PanelEvent, panel_header_height, render_panel_header_controls_at},
};

use crate::git_graph::GitGraph;

const GIT_LOG_PANEL_KEY: &str = "GitLogPanel";
const BRANCHES_WIDTH: f32 = 220.;

actions!(
    git_log_panel,
    [
        /// Toggles focus on the Git tool window, which shows the commit log in the islands layout.
        ToggleFocus
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(
        |workspace: &mut Workspace, _window, _: &mut Context<Workspace>| {
            workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
                workspace.toggle_panel_focus::<GitLogPanel>(window, cx);
            });
        },
    )
    .detach();
}

/// Opens the Git tool window instead of a Git Graph tab when the islands layout is on.
pub(crate) fn reveal(
    workspace: &mut Workspace,
    sha: Option<String>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> bool {
    if !WorkspaceSettings::get_global(cx).islands.enabled {
        return false;
    }
    let Some(panel) = workspace.panel::<GitLogPanel>(cx) else {
        return false;
    };
    panel.update(cx, |panel, cx| panel.sync_graph(window, cx));
    workspace.open_panel::<GitLogPanel>(window, cx);
    workspace.focus_panel::<GitLogPanel>(window, cx);
    if let Some(sha) = sha {
        cx.defer(move |cx| {
            panel.update(cx, |panel, cx| {
                if let Some(graph) = panel.graph.clone() {
                    graph.update(cx, |graph, cx| graph.select_commit_by_sha(sha.as_str(), cx));
                }
            });
        });
    }
    true
}

#[derive(Clone, PartialEq, Eq)]
enum BranchRow {
    Head,
    Group(SharedString),
    Branch(SharedString),
}

/// IntelliJ's Git tool window: a branches tree beside the commit log, in the bottom dock.
pub struct GitLogPanel {
    workspace: WeakEntity<Workspace>,
    git_store: Entity<GitStore>,
    focus_handle: FocusHandle,
    position: DockPosition,
    graph: Option<Entity<GitGraph>>,
    graph_repository: Option<RepositoryId>,
    selected: Option<BranchRow>,
    collapsed_groups: HashSet<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl GitLogPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> anyhow::Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            let git_store = workspace.project().read(cx).git_store().clone();
            let workspace = cx.entity().downgrade();
            cx.new(|cx| {
                let subscription = cx.subscribe_in(
                    &git_store,
                    window,
                    |this: &mut Self, _, event, window, cx| match event {
                        GitStoreEvent::ActiveRepositoryChanged(_)
                        | GitStoreEvent::RepositoryAdded
                        | GitStoreEvent::RepositoryRemoved(_) => {
                            if this.graph.is_some() {
                                this.sync_graph(window, cx);
                            }
                            cx.notify();
                        }
                        GitStoreEvent::RepositoryUpdated(_, _, true) => cx.notify(),
                        _ => {}
                    },
                );
                let panel = Self {
                    workspace,
                    git_store,
                    focus_handle: cx.focus_handle(),
                    position: DockPosition::Bottom,
                    graph: None,
                    graph_repository: None,
                    selected: None,
                    collapsed_groups: HashSet::default(),
                    _subscriptions: vec![subscription],
                };
                panel
            })
        })
    }

    fn active_repository(&self, cx: &App) -> Option<Entity<Repository>> {
        self.git_store.read(cx).active_repository()
    }

    fn sync_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repository_id = self
            .active_repository(cx)
            .map(|repository| repository.read(cx).id);
        if repository_id == self.graph_repository {
            return;
        }
        self.graph_repository = repository_id;
        self.selected = None;
        self.graph = repository_id.map(|repository_id| {
            let git_store = self.git_store.clone();
            let workspace = self.workspace.clone();
            cx.new(|cx| GitGraph::new(repository_id, git_store, workspace, None, window, cx))
        });
        cx.notify();
    }

    fn branches(&self, cx: &App) -> Arc<[Branch]> {
        self.active_repository(cx)
            .map(|repository| repository.read(cx).branch_list.clone())
            .unwrap_or_else(|| Arc::from([]))
    }

    fn rows(&self, cx: &App) -> Vec<(BranchRow, usize)> {
        let branches = self.branches(cx);
        let mut rows = vec![(BranchRow::Head, 0)];

        let local_group: SharedString = "Local".into();
        rows.push((BranchRow::Group(local_group.clone()), 0));
        if !self.collapsed_groups.contains(&local_group) {
            let mut local = branches
                .iter()
                .filter(|branch| !branch.is_remote())
                .map(|branch| SharedString::from(branch.ref_name.to_string()))
                .collect::<Vec<_>>();
            local.sort_by_key(|ref_name| branch_display_name(ref_name).to_string());
            rows.extend(
                local
                    .into_iter()
                    .map(|ref_name| (BranchRow::Branch(ref_name), 1)),
            );
        }

        let mut remotes = branches
            .iter()
            .filter_map(|branch| branch.remote_name().map(|name| name.to_string()))
            .collect::<Vec<_>>();
        remotes.sort();
        remotes.dedup();
        if remotes.is_empty() {
            return rows;
        }
        let remote_group: SharedString = "Remote".into();
        rows.push((BranchRow::Group(remote_group.clone()), 0));
        if self.collapsed_groups.contains(&remote_group) {
            return rows;
        }
        for remote in remotes {
            let group: SharedString = format!("remote:{remote}").into();
            rows.push((BranchRow::Group(group.clone()), 1));
            if self.collapsed_groups.contains(&group) {
                continue;
            }
            let mut remote_branches = branches
                .iter()
                .filter(|branch| branch.remote_name() == Some(remote.as_str()))
                .map(|branch| SharedString::from(branch.ref_name.to_string()))
                .collect::<Vec<_>>();
            remote_branches.sort();
            rows.extend(
                remote_branches
                    .into_iter()
                    .map(|ref_name| (BranchRow::Branch(ref_name), 2)),
            );
        }
        rows
    }

    fn activate_row(&mut self, row: BranchRow, cx: &mut Context<Self>) {
        let branches = self.branches(cx);
        let sha = match &row {
            BranchRow::Group(group) => {
                if !self.collapsed_groups.remove(group) {
                    self.collapsed_groups.insert(group.clone());
                }
                cx.notify();
                return;
            }
            BranchRow::Head => branches
                .iter()
                .find(|branch| branch.is_head)
                .and_then(|branch| branch.most_recent_commit.as_ref())
                .map(|commit| commit.sha.clone()),
            BranchRow::Branch(ref_name) => branches
                .iter()
                .find(|branch| branch.ref_name == *ref_name)
                .and_then(|branch| branch.most_recent_commit.as_ref())
                .map(|commit| commit.sha.clone()),
        };
        self.selected = Some(row);
        if let (Some(sha), Some(graph)) = (sha, self.graph.clone()) {
            graph.update(cx, |graph, cx| graph.select_commit_by_sha(sha.as_ref(), cx));
        }
        cx.notify();
    }

    fn render_header(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let radius = ui::islands_control_radius(cx);
        let active_background = cx.theme().colors().ghost_element_active;
        h_flex()
            .id("git-log-panel-header")
            .flex_none()
            .h(panel_header_height(cx))
            .w_full()
            .pl_3()
            .pr_1()
            .gap_0p5()
            .child(Label::new("Git").weight(FontWeight::SEMIBOLD).mr_2())
            .child(
                div()
                    .px_2()
                    .py_0p5()
                    .rounded(radius)
                    .bg(active_background)
                    .child(Label::new("Log")),
            )
            .child(div().flex_1())
            .child(render_panel_header_controls_at(
                Arc::new(cx.entity()),
                self.position(window, cx),
                "Git".into(),
            ))
    }

    fn render_branches(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let branches = self.branches(cx);
        let radius = ui::islands_control_radius(cx);
        let selected_background = cx.theme().colors().ghost_element_selected;
        let hover_background = cx.theme().colors().ghost_element_hover;
        let border_color = cx.theme().colors().border_variant;

        let rows = self
            .rows(cx)
            .into_iter()
            .enumerate()
            .map(|(index, (row, depth))| {
                let selected = self.selected.as_ref() == Some(&row);
                let content = match &row {
                    BranchRow::Head => h_flex()
                        .gap_1p5()
                        .child(
                            Icon::new(IconName::GitBranch)
                                .size(IconSize::Small)
                                .color(Color::Accent),
                        )
                        .child(Label::new("HEAD (Current Branch)")),
                    BranchRow::Group(group) => {
                        let collapsed = self.collapsed_groups.contains(group);
                        let label = group
                            .strip_prefix("remote:")
                            .map(|remote| remote.to_string())
                            .unwrap_or_else(|| group.to_string());
                        h_flex()
                            .gap_1()
                            .child(
                                Icon::new(if collapsed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                })
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                            )
                            .when(depth > 0, |this| {
                                this.child(
                                    Icon::new(IconName::Server)
                                        .size(IconSize::Small)
                                        .color(Color::Muted),
                                )
                            })
                            .child(Label::new(label))
                    }
                    BranchRow::Branch(ref_name) => {
                        let is_head = branches
                            .iter()
                            .any(|branch| branch.is_head && branch.ref_name == *ref_name);
                        h_flex()
                            .gap_1p5()
                            .child(
                                Icon::new(if is_head {
                                    IconName::Star
                                } else {
                                    IconName::GitBranch
                                })
                                .size(IconSize::Small)
                                .color(if is_head {
                                    Color::Accent
                                } else {
                                    Color::Muted
                                }),
                            )
                            .child(Label::new(branch_display_name(ref_name).to_string()).truncate())
                    }
                };
                h_flex()
                    .id(("git-log-branch", index))
                    .w_full()
                    .h_6()
                    .pl(px(8. + 14. * depth as f32))
                    .pr_2()
                    .rounded(radius)
                    .cursor_pointer()
                    .overflow_hidden()
                    .map(|this| {
                        if selected {
                            this.bg(selected_background)
                        } else {
                            this.hover(move |style| style.bg(hover_background))
                        }
                    })
                    .child(content)
                    .on_click(cx.listener(move |this, _, _, cx| this.activate_row(row.clone(), cx)))
            });

        v_flex()
            .id("git-log-branches")
            .flex_none()
            .w(px(BRANCHES_WIDTH))
            .h_full()
            .p_1()
            .border_r_1()
            .border_color(border_color)
            .overflow_y_scroll()
            .children(rows)
    }
}

fn branch_display_name(ref_name: &str) -> &str {
    ref_name
        .strip_prefix("refs/heads/")
        .or_else(|| {
            ref_name
                .strip_prefix("refs/remotes/")
                .and_then(|rest| rest.split_once('/').map(|(_, name)| name))
        })
        .unwrap_or(ref_name)
}

impl EventEmitter<PanelEvent> for GitLogPanel {}

impl Focusable for GitLogPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.graph {
            Some(graph) => graph.focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

impl Render for GitLogPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The log is loaded on first show so that a hidden tool window costs nothing.
        self.sync_graph(window, cx);
        let body = match self.graph.clone() {
            Some(graph) => h_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(self.render_branches(cx))
                .child(div().flex_1().min_w_0().h_full().child(graph))
                .into_any_element(),
            None => v_flex()
                .flex_1()
                .w_full()
                .items_center()
                .justify_center()
                .child(Label::new("No Git repository in this project").color(Color::Muted))
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .child(self.render_header(window, cx))
            .child(body)
    }
}

impl Panel for GitLogPanel {
    fn persistent_name() -> &'static str {
        "GitLogPanel"
    }

    fn panel_key() -> &'static str {
        GIT_LOG_PANEL_KEY
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
            DockPosition::Left | DockPosition::Right => px(480.),
            DockPosition::Bottom => px(320.),
        }
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::IslandsGit)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Git")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        10
    }

    fn enabled(&self, cx: &App) -> bool {
        WorkspaceSettings::get_global(cx).islands.enabled && self.active_repository(cx).is_some()
    }

    fn draws_own_islands_header(&self) -> bool {
        true
    }
}
