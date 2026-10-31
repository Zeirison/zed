use std::{collections::HashMap, ops::Range, sync::Arc};

use collections::HashSet;
use editor::{Editor, ReferencesPresenter};
use file_icons::FileIcons;
use gpui::{
    Action, App, AsyncWindowContext, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, KeyContext, ListSizingBehavior, Pixels, Render, ScrollStrategy,
    UniformListScrollHandle, WeakEntity, Window, actions, uniform_list,
};
use language::{Buffer, Point};
use project::ProjectPath;
use settings::Settings;
use ui::{HighlightedLabel, prelude::*};
use workspace::{
    Workspace, WorkspaceSettings,
    dock::{DockPosition, Panel, PanelEvent, panel_header_height, render_panel_header_controls_at},
};

const FIND_PANEL_KEY: &str = "FindPanel";
const MAX_LINE_LENGTH: usize = 200;

actions!(
    find_panel,
    [
        /// Toggles focus on the Find tool window, which lists "find all references" results
        /// in the islands layout.
        ToggleFocus
    ]
);

pub fn init(cx: &mut App) {
    cx.set_global(ReferencesPresenter(Arc::new(
        |workspace, symbol, locations, window, cx| {
            if !WorkspaceSettings::get_global(cx).islands.enabled {
                return false;
            }
            let Some(panel) = workspace.panel::<FindPanel>(cx) else {
                return false;
            };
            panel.update(cx, |panel, cx| panel.add_search(symbol, locations, cx));
            workspace.open_panel::<FindPanel>(window, cx);
            workspace.focus_panel::<FindPanel>(window, cx);
            true
        },
    )));
    cx.observe_new(
        |workspace: &mut Workspace, _window, _: &mut Context<Workspace>| {
            workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
                workspace.toggle_panel_focus::<FindPanel>(window, cx);
            });
        },
    )
    .detach();
}

struct Usage {
    position: Point,
    line_text: SharedString,
    highlights: Vec<usize>,
}

struct FileUsages {
    path: ProjectPath,
    usages: Vec<Usage>,
}

struct Search {
    title: SharedString,
    files: Vec<FileUsages>,
    collapsed: HashSet<usize>,
}

impl Search {
    fn usage_count(&self) -> usize {
        self.files.iter().map(|file| file.usages.len()).sum()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FindRow {
    File(usize),
    Usage(usize, usize),
}

/// IntelliJ's Find tool window: each "find all references" run becomes a tab
/// whose usages are grouped by file, in the bottom dock of the islands layout.
pub struct FindPanel {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    position: DockPosition,
    searches: Vec<Search>,
    active_search: usize,
    selected: Option<FindRow>,
}

impl FindPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> anyhow::Result<Entity<Self>> {
        workspace.update_in(&mut cx, |_, _, cx| {
            let workspace = cx.entity().downgrade();
            cx.new(|cx| Self {
                workspace,
                focus_handle: cx.focus_handle(),
                scroll_handle: UniformListScrollHandle::new(),
                position: DockPosition::Bottom,
                searches: Vec::new(),
                active_search: 0,
                selected: None,
            })
        })
    }

    fn add_search(
        &mut self,
        symbol: SharedString,
        locations: HashMap<Entity<Buffer>, Vec<Range<Point>>>,
        cx: &mut Context<Self>,
    ) {
        let mut files = locations
            .into_iter()
            .filter_map(|(buffer, ranges)| {
                let buffer = buffer.read(cx);
                let file = buffer.file()?;
                let path = ProjectPath {
                    worktree_id: file.worktree_id(cx),
                    path: file.path().clone(),
                };
                let snapshot = buffer.snapshot();
                let mut ranges = ranges;
                ranges.sort_by_key(|range| range.start);
                let usages = ranges
                    .into_iter()
                    .map(|range| {
                        let row = range.start.row;
                        let line_end = Point::new(row, snapshot.line_len(row));
                        let line = snapshot
                            .text_for_range(Point::new(row, 0)..line_end)
                            .collect::<String>();
                        usage_for_line(&line, range, row)
                    })
                    .collect::<Vec<_>>();
                Some(FileUsages { path, usages })
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| left.path.path.cmp(&right.path.path));

        let title: SharedString = format!("Usages of {symbol}").into();
        // Like IntelliJ, searching the same symbol again replaces its tab.
        self.searches.retain(|search| search.title != title);
        self.searches.push(Search {
            title,
            files,
            collapsed: HashSet::default(),
        });
        self.active_search = self.searches.len() - 1;
        self.selected = None;
        if let Some(row) = self
            .rows()
            .into_iter()
            .find(|row| matches!(row, FindRow::Usage(..)))
        {
            self.select(row, cx);
        }
        cx.notify();
    }

    fn active(&self) -> Option<&Search> {
        self.searches.get(self.active_search)
    }

    fn rows(&self) -> Vec<FindRow> {
        let Some(search) = self.active() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (file_index, file) in search.files.iter().enumerate() {
            rows.push(FindRow::File(file_index));
            if !search.collapsed.contains(&file_index) {
                rows.extend((0..file.usages.len()).map(|index| FindRow::Usage(file_index, index)));
            }
        }
        rows
    }

    fn set_active_search(&mut self, index: usize, cx: &mut Context<Self>) {
        self.active_search = index;
        self.selected = None;
        cx.notify();
    }

    fn close_search(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.searches.len() {
            self.searches.remove(index);
        }
        self.active_search = self
            .active_search
            .min(self.searches.len().saturating_sub(1));
        self.selected = None;
        if self.searches.is_empty() {
            cx.emit(PanelEvent::Close);
        }
        cx.notify();
    }

    fn select(&mut self, row: FindRow, cx: &mut Context<Self>) {
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
            Some(FindRow::File(file_index)) | Some(FindRow::Usage(file_index, _)) => file_index,
            None => return,
        };
        if let Some(search) = self.searches.get_mut(self.active_search) {
            search.collapsed.insert(file_index);
        }
        self.select(FindRow::File(file_index), cx);
    }

    fn expand_selected(&mut self, _: &menu::SelectChild, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(FindRow::File(file_index)) = self.selected
            && let Some(search) = self.searches.get_mut(self.active_search)
        {
            search.collapsed.remove(&file_index);
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected {
            Some(FindRow::Usage(file_index, index)) => {
                self.open_usage(file_index, index, true, window, cx)
            }
            Some(FindRow::File(file_index)) => self.toggle_collapsed(file_index, cx),
            None => {}
        }
    }

    fn toggle_collapsed(&mut self, file_index: usize, cx: &mut Context<Self>) {
        if let Some(search) = self.searches.get_mut(self.active_search)
            && !search.collapsed.remove(&file_index)
        {
            search.collapsed.insert(file_index);
        }
        cx.notify();
    }

    fn open_usage(
        &mut self,
        file_index: usize,
        index: usize,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((path, position)) = self.active().and_then(|search| {
            let file = search.files.get(file_index)?;
            let usage = file.usages.get(index)?;
            Some((file.path.clone(), usage.position))
        }) else {
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
        let active_background = cx.theme().colors().ghost_element_active;
        let hover_background = cx.theme().colors().ghost_element_hover;
        let tabs = self
            .searches
            .iter()
            .enumerate()
            .map(|(index, search)| {
                let active = index == self.active_search;
                h_flex()
                    .id(("find-tab", index))
                    .gap_1()
                    .pl_2()
                    .pr_0p5()
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
                    .child(Label::new(search.title.clone()))
                    .child(Label::new(search.usage_count().to_string()).color(Color::Muted))
                    .child(
                        IconButton::new(("close-find-tab", index), IconName::Close)
                            .icon_size(IconSize::XSmall)
                            .icon_color(Color::Muted)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_search(index, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.set_active_search(index, cx)))
            })
            .collect::<Vec<_>>();

        h_flex()
            .id("find-panel-header")
            .flex_none()
            .h(panel_header_height(cx))
            .w_full()
            .pl_3()
            .pr_1()
            .gap_0p5()
            .child(Label::new("Find").weight(FontWeight::SEMIBOLD).mr_2())
            .child(
                h_flex()
                    .gap_0p5()
                    .min_w_0()
                    .overflow_hidden()
                    .children(tabs),
            )
            .child(div().flex_1())
            .child(render_panel_header_controls_at(
                Arc::new(cx.entity()),
                self.position(window, cx),
                "Find".into(),
            ))
    }

    fn render_row(&self, row: FindRow, row_index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(search) = self.active() else {
            return div().into_any_element();
        };
        let selected = self.selected == Some(row);
        let selected_background = cx.theme().colors().ghost_element_selected;
        let hover_background = cx.theme().colors().ghost_element_hover;
        let radius = ui::islands_control_radius(cx);
        let path_style = self
            .workspace
            .upgrade()
            .map(|workspace| workspace.read(cx).project().read(cx).path_style(cx));
        let base = h_flex()
            .id(("find-row", row_index))
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
            FindRow::File(file_index) => {
                let Some(file) = search.files.get(file_index) else {
                    return div().into_any_element();
                };
                let collapsed = search.collapsed.contains(&file_index);
                let path = &file.path.path;
                let file_name = path
                    .file_name()
                    .map(|name| name.to_string())
                    .unwrap_or_default();
                let directory = path_style.and_then(|path_style| {
                    path.parent()
                        .filter(|parent| !parent.is_empty())
                        .map(|parent| parent.display(path_style).to_string())
                });
                let file_icon = FileIcons::get_icon(path.as_std_path(), cx);
                let count = file.usages.len();
                let count_label = if count == 1 {
                    "1 usage".to_string()
                } else {
                    format!("{count} usages")
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
            FindRow::Usage(file_index, index) => {
                let Some(usage) = search
                    .files
                    .get(file_index)
                    .and_then(|file| file.usages.get(index))
                else {
                    return div().into_any_element();
                };
                base.pl(px(28.))
                    .child(
                        div().flex_none().w(px(36.)).child(
                            Label::new((usage.position.row + 1).to_string())
                                .color(Color::Muted)
                                .buffer_font(cx),
                        ),
                    )
                    .child(
                        HighlightedLabel::new(usage.line_text.clone(), usage.highlights.clone())
                            .buffer_font(cx)
                            .truncate(),
                    )
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        this.selected = Some(row);
                        let focus_editor = event.click_count() > 1;
                        this.open_usage(file_index, index, focus_editor, window, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            }
        }
    }

    fn dispatch_context(&self) -> KeyContext {
        let mut context = KeyContext::default();
        context.add("FindPanel");
        context.add("menu");
        context
    }
}

/// Trims a usage's line for display and returns the byte offsets of the usage in it.
fn usage_for_line(line: &str, range: Range<Point>, row: u32) -> Usage {
    let indent = line.len() - line.trim_start().len();
    let text = &line[indent..];
    let text = match text.char_indices().nth(MAX_LINE_LENGTH) {
        Some((end, _)) => &text[..end],
        None => text,
    };
    let start = (range.start.column as usize).saturating_sub(indent);
    let end = if range.end.row == row {
        (range.end.column as usize).saturating_sub(indent)
    } else {
        text.len()
    };
    let highlights = text
        .char_indices()
        .map(|(offset, _)| offset)
        .filter(|offset| (start..end).contains(offset))
        .collect();
    Usage {
        position: range.start,
        line_text: text.to_string().into(),
        highlights,
    }
}

impl EventEmitter<PanelEvent> for FindPanel {}

impl Focusable for FindPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FindPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let row_count = self.rows().len();
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
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .child(
                        Label::new("Find usages of a symbol to see them here").color(Color::Muted),
                    )
                    .into_any_element()
            } else {
                uniform_list(
                    "find-usages",
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

impl Panel for FindPanel {
    fn persistent_name() -> &'static str {
        "FindPanel"
    }

    fn panel_key() -> &'static str {
        FIND_PANEL_KEY
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
        Some(IconName::IslandsFind)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Find")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        11
    }

    /// Like IntelliJ, the Find tool window only appears once there is something to show.
    fn enabled(&self, cx: &App) -> bool {
        WorkspaceSettings::get_global(cx).islands.enabled && !self.searches.is_empty()
    }

    fn draws_own_islands_header(&self) -> bool {
        true
    }
}
