use std::sync::Arc;

use file_icons::FileIcons;
use gpui::{
    Action, App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    Render, Task, WeakEntity, Window, actions, px,
};
use picker::{Picker, PickerDelegate};
use project::ProjectPath;
use settings::Settings;
use ui::{ListItem, ListItemSpacing, prelude::*};
use util::ResultExt;
use workspace::{ModalView, Workspace, WorkspaceSettings, dock::PanelHandle};

const MAX_RECENT_FILES: usize = 50;
const TOOL_WINDOWS_WIDTH: f32 = 180.;

actions!(
    recent_files,
    [
        /// Shows IntelliJ's Recent Files popup in the islands layout, or the file finder otherwise.
        Toggle
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(
        |workspace: &mut Workspace, _window, _: &mut Context<Workspace>| {
            workspace.register_action(|workspace, _: &Toggle, window, cx| {
                if !WorkspaceSettings::get_global(cx).islands.enabled {
                    window.dispatch_action(Box::new(workspace::ToggleFileFinder::default()), cx);
                    return;
                }
                RecentFiles::toggle(workspace, window, cx);
            });
        },
    )
    .detach();
}

struct ToolWindow {
    icon: IconName,
    name: &'static str,
    action: Box<dyn Action>,
}

/// IntelliJ's Recent Files popup: tool windows on the left, recently opened files on the right.
pub struct RecentFiles {
    picker: Entity<Picker<RecentFilesDelegate>>,
    panels: Vec<Arc<dyn PanelHandle>>,
    tool_windows: Vec<ToolWindow>,
}

impl ModalView for RecentFiles {}

impl EventEmitter<DismissEvent> for RecentFiles {}

impl Focusable for RecentFiles {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl RecentFiles {
    fn toggle(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        if let Some(existing) = workspace.active_modal::<Self>(cx) {
            existing.update(cx, |this, cx| {
                this.picker
                    .update(cx, |picker, cx| picker.cycle_selection(window, cx))
            });
            return;
        }

        let active_path = workspace
            .active_item(cx)
            .and_then(|item| item.project_path(cx));
        let mut files = Vec::new();
        files.extend(active_path.clone());
        files.extend(
            workspace
                .recent_navigation_history_iter(cx)
                .map(|(path, _)| path)
                .filter(|path| Some(path) != active_path.as_ref())
                .take(MAX_RECENT_FILES),
        );

        // Panels such as the terminal read the workspace to describe themselves, so they are
        // described when the popup renders rather than here, where the workspace is borrowed.
        let panels = [
            workspace.left_dock(),
            workspace.bottom_dock(),
            workspace.right_dock(),
        ]
        .into_iter()
        .flat_map(|dock| dock.read(cx).panels().cloned().collect::<Vec<_>>())
        .collect::<Vec<_>>();

        let workspace_handle = cx.entity().downgrade();
        let path_style = workspace.project().read(cx).path_style(cx);
        workspace.toggle_modal(window, cx, |window, cx| {
            let recent_files = cx.entity().downgrade();
            let delegate = RecentFilesDelegate {
                recent_files,
                workspace: workspace_handle,
                path_style,
                // Like IntelliJ, start on the previous file so that Ctrl+E, Enter switches back.
                selected_index: usize::from(files.len() > 1),
                matches: (0..files.len()).collect(),
                files,
            };
            let picker = cx.new(|cx| Picker::uniform_list(delegate, window, cx).embedded());
            Self {
                picker,
                panels,
                tool_windows: Vec::new(),
            }
        });
    }

    fn describe_tool_windows(&self, window: &Window, cx: &App) -> Vec<ToolWindow> {
        let mut tool_windows = self
            .panels
            .iter()
            .enumerate()
            .filter(|(_, panel)| panel.enabled(cx))
            .filter_map(|(order, panel)| {
                let position = panel.position(window, cx) as usize;
                Some((
                    (position, panel.activation_priority(cx), order),
                    ToolWindow {
                        icon: panel.icon(window, cx)?,
                        name: panel.icon_tooltip(window, cx)?,
                        action: panel.toggle_action(window, cx),
                    },
                ))
            })
            .collect::<Vec<_>>();
        tool_windows.sort_by_key(|(key, _)| *key);
        tool_windows
            .into_iter()
            .map(|(_, tool_window)| tool_window)
            .collect()
    }

    fn open_tool_window(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tool_window) = self.tool_windows.get(index) else {
            return;
        };
        let action = tool_window.action.boxed_clone();
        cx.emit(DismissEvent);
        window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
    }

    fn render_tool_windows(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let radius = ui::islands_control_radius(cx);
        let hover_background = cx.theme().colors().ghost_element_hover;
        let border_color = cx.theme().colors().border_variant;
        v_flex()
            .id("recent-files-tool-windows")
            .flex_none()
            .w(px(TOOL_WINDOWS_WIDTH))
            .h_full()
            .p_1()
            .border_r_1()
            .border_color(border_color)
            .overflow_y_scroll()
            .children(
                self.tool_windows
                    .iter()
                    .enumerate()
                    .map(|(index, tool_window)| {
                        h_flex()
                            .id(("recent-files-tool-window", index))
                            .h_6()
                            .px_2()
                            .gap_2()
                            .rounded(radius)
                            .cursor_pointer()
                            .hover(move |style| style.bg(hover_background))
                            .child(
                                Icon::new(tool_window.icon)
                                    .size(IconSize::Small)
                                    .color(Color::Muted),
                            )
                            // Zed names some panels "X Panel"; IntelliJ lists tool windows by their bare name.
                            .child(
                                Label::new(tool_window.name.trim_end_matches(" Panel")).truncate(),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_tool_window(index, window, cx)
                            }))
                    }),
            )
    }
}

impl Render for RecentFiles {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.tool_windows = self.describe_tool_windows(window, cx);
        let colors = cx.theme().colors();
        let radius = ui::islands_style(cx).corner_radius;
        v_flex()
            .key_context("RecentFiles")
            .w(px(720.))
            .max_h(px(420.))
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(colors.border)
            .rounded(radius)
            .shadow_lg()
            .overflow_hidden()
            .child(
                h_flex()
                    .flex_none()
                    .h_8()
                    .px_3()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(Label::new("Recent Files").weight(FontWeight::SEMIBOLD)),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(self.render_tool_windows(cx))
                    .child(div().flex_1().min_w_0().h_full().child(self.picker.clone())),
            )
    }
}

pub struct RecentFilesDelegate {
    recent_files: WeakEntity<RecentFiles>,
    workspace: WeakEntity<Workspace>,
    path_style: util::paths::PathStyle,
    files: Vec<ProjectPath>,
    matches: Vec<usize>,
    selected_index: usize,
}

impl RecentFilesDelegate {
    fn search_text(&self, path: &ProjectPath) -> String {
        path.path.display(self.path_style).to_lowercase()
    }
}

/// Matches when the query's characters appear in order, so "mrs" finds "main.rs".
fn is_subsequence(query: &str, text: &str) -> bool {
    let mut text_chars = text.chars();
    query
        .chars()
        .filter(|character| !character.is_whitespace())
        .all(|query_char| text_chars.any(|text_char| text_char == query_char))
}

impl PickerDelegate for RecentFilesDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "recent files"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "Type to search recent files".into()
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        Some("No recent files".into())
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(
        &mut self,
        index: usize,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) {
        self.selected_index = index;
    }

    fn update_matches(
        &mut self,
        query: String,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let query = query.to_lowercase();
        if query.trim().is_empty() {
            self.matches = (0..self.files.len()).collect();
            self.selected_index = usize::from(self.files.len() > 1);
        } else {
            self.matches = self
                .files
                .iter()
                .enumerate()
                .filter(|(_, path)| is_subsequence(&query, &self.search_text(path)))
                .map(|(index, _)| index)
                .collect();
            self.selected_index = 0;
        }
        Task::ready(())
    }

    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(path) = self
            .matches
            .get(self.selected_index)
            .and_then(|index| self.files.get(*index))
            .cloned()
        else {
            return;
        };
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                let task = if secondary {
                    workspace.split_path(path, window, cx)
                } else {
                    workspace.open_path(path, None, true, window, cx)
                };
                task.detach_and_log_err(cx);
            });
        }
        self.dismissed(window, cx);
    }

    fn dismissed(&mut self, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.recent_files
            .update(cx, |_, cx| cx.emit(DismissEvent))
            .log_err();
    }

    fn render_match(
        &self,
        index: usize,
        selected: bool,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let path = self.files.get(*self.matches.get(index)?)?;
        let file_name = path
            .path
            .file_name()
            .map(|name| name.to_string())
            .unwrap_or_else(|| path.path.display(self.path_style).to_string());
        let directory = path
            .path
            .parent()
            .filter(|parent| !parent.is_empty())
            .map(|parent| parent.display(self.path_style).to_string());
        let icon = FileIcons::get_icon(path.path.as_std_path(), cx)
            .map(|icon_path| Icon::from_path(icon_path).color(Color::Muted));
        Some(
            ListItem::new(index)
                .spacing(ListItemSpacing::Sparse)
                .inset(true)
                .toggle_state(selected)
                .start_slot::<Icon>(icon)
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .child(Label::new(file_name))
                        .when_some(directory, |this, directory| {
                            this.child(
                                Label::new(directory)
                                    .color(Color::Muted)
                                    .size(LabelSize::Small)
                                    .truncate(),
                            )
                        }),
                ),
        )
    }
}
