use gpui::{
    Action, AnyElement, App, Context, Empty, EventEmitter, IntoElement, ParentElement, Render,
    SharedString, Subscription, WeakEntity, Window,
};
use project::Project;
use settings::Settings;
use ui::{Button, Tooltip, prelude::*};
use util::paths::PathStyle;

use crate::{
    HideStatusItem, StatusItemView, WorkspaceSettings,
    item::{ItemEvent, ItemHandle},
    pane::RevealInProjectPanel,
    workspace_settings::StatusBarSettings,
};

pub struct ActiveFileName {
    project_path: Option<SharedString>,
    full_path: Option<SharedString>,
    project: Option<WeakEntity<Project>>,
    active_item: Option<Box<dyn ItemHandle>>,
    _item_subscription: Option<Subscription>,
}

impl ActiveFileName {
    pub fn new() -> Self {
        Self {
            project_path: None,
            full_path: None,
            project: None,
            active_item: None,
            _item_subscription: None,
        }
    }

    pub fn with_project(mut self, project: WeakEntity<Project>) -> Self {
        self.project = Some(project);
        self
    }

    /// Renders the IDEA-style navigation bar: the project, the folders and the
    /// file of the active item, followed by the symbols at the cursor.
    fn render_navigation_path(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let item = self.active_item.as_ref()?;
        let project_path = item.project_path(cx)?;
        let root_name = self
            .project
            .as_ref()
            .and_then(|project| project.upgrade())
            .and_then(|project| {
                project
                    .read(cx)
                    .worktree_for_id(project_path.worktree_id, cx)
            })
            .map(|worktree| SharedString::from(worktree.read(cx).root_name_str().to_string()));
        let path_segments: Vec<SharedString> = root_name
            .into_iter()
            .chain(
                project_path
                    .path
                    .components()
                    .map(|component| SharedString::from(component.to_string())),
            )
            .collect();
        // The first breadcrumb of a file is its path, which is already covered
        // by the path segments.
        let symbol_segments: Vec<SharedString> = item
            .breadcrumbs(cx)
            .map(|(segments, _)| {
                segments
                    .into_iter()
                    .skip(1)
                    .map(|segment| segment.text)
                    .collect()
            })
            .unwrap_or_default();

        let path_count = path_segments.len();
        let segment_count = path_count + symbol_segments.len();
        let segments = path_segments
            .into_iter()
            .chain(symbol_segments)
            .enumerate()
            .map(|(index, label)| {
                let is_symbol = index >= path_count;
                let is_last = index + 1 == segment_count;
                let (tooltip, action): (&str, Option<Box<dyn Action>>) = if is_symbol {
                    (
                        "Go to Symbol",
                        cx.build_action("outline::Toggle", None).ok(),
                    )
                } else {
                    (
                        "Reveal in Project Panel",
                        Some(RevealInProjectPanel::default().boxed_clone()),
                    )
                };
                h_flex()
                    .when(index > 0, |this| {
                        this.child(
                            Icon::new(IconName::ChevronRight)
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
                    .child(
                        Button::new(("navigation-path-segment", index), label)
                            .label_size(LabelSize::Small)
                            .color(if is_last {
                                Color::Default
                            } else {
                                Color::Muted
                            })
                            .tooltip(Tooltip::text(tooltip))
                            .when_some(action, |this, action| {
                                this.on_click(move |_, window, cx| {
                                    window.dispatch_action(action.boxed_clone(), cx)
                                })
                            }),
                    )
            });

        Some(
            h_flex()
                .gap_0p5()
                .overflow_hidden()
                .children(segments)
                .into_any_element(),
        )
    }
}

impl Render for ActiveFileName {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if WorkspaceSettings::get_global(cx)
            .islands
            .show_navigation_path()
        {
            return self
                .render_navigation_path(cx)
                .unwrap_or_else(|| Empty.into_any_element());
        }

        if !StatusBarSettings::get_global(cx).show_active_file {
            return Empty.into_any_element();
        }

        let Some(project_path) = self.project_path.clone() else {
            return Empty.into_any_element();
        };

        let tooltip_text = self
            .full_path
            .clone()
            .unwrap_or_else(|| project_path.clone());

        div()
            .child(
                Button::new("active-file-name-button", project_path)
                    .label_size(LabelSize::Small)
                    .tooltip(Tooltip::text(tooltip_text)),
            )
            .into_any_element()
    }
}

impl EventEmitter<crate::ToolbarItemEvent> for ActiveFileName {}

impl StatusItemView for ActiveFileName {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_item = active_pane_item.map(|item| item.boxed_clone());
        self._item_subscription = active_pane_item.map(|item| {
            let this = cx.entity().downgrade();
            item.subscribe_to_item_events(
                window,
                cx,
                Box::new(move |event, _, cx| {
                    if let ItemEvent::UpdateBreadcrumbs = event {
                        this.update(cx, |_, cx| cx.notify()).ok();
                    }
                }),
            )
        });
        if let Some(item) = active_pane_item {
            self.project_path = item
                .project_path(cx)
                .map(|path| path.path.display(PathStyle::local()).into_owned().into());
            self.full_path = item.tab_tooltip_text(cx);
        } else {
            self.project_path = None;
            self.full_path = None;
        }
        cx.notify();
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        Some(HideStatusItem::new(|settings| {
            settings.status_bar.get_or_insert_default().show_active_file = Some(false);
        }))
    }
}
