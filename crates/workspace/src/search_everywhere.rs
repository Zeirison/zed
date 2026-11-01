use std::rc::Rc;

use collections::HashMap;
use gpui::{
    Action, AnyElement, App, Context, Global, IntoElement, ParentElement, Styled, Window, actions,
};
use schemars::JsonSchema;
use serde::Deserialize;
use ui::{
    ButtonCommon, ButtonSize, ButtonStyle, Clickable, Color, Label, LabelSize, SelectableButton,
    Toggleable, prelude::*,
};

use crate::Workspace;

/// One of the pickers that IntelliJ-style Search Everywhere switches between.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchEverywhereTab {
    Files,
    Symbols,
    Actions,
    Text,
}

impl SearchEverywhereTab {
    pub const ALL: [SearchEverywhereTab; 4] = [
        SearchEverywhereTab::Files,
        SearchEverywhereTab::Symbols,
        SearchEverywhereTab::Actions,
        SearchEverywhereTab::Text,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            SearchEverywhereTab::Files => "Files",
            SearchEverywhereTab::Symbols => "Symbols",
            SearchEverywhereTab::Actions => "Actions",
            SearchEverywhereTab::Text => "Text",
        }
    }
}

actions!(
    search_everywhere,
    [
        /// Opens Search Everywhere on the tab that was used last.
        Toggle,
        /// Switches Search Everywhere to the next tab, keeping the query.
        NextTab,
        /// Switches Search Everywhere to the previous tab, keeping the query.
        PreviousTab
    ]
);

/// Opens a Search Everywhere tab with the given query.
#[derive(Clone, PartialEq, Eq, Deserialize, JsonSchema, Action)]
#[action(namespace = search_everywhere)]
#[serde(deny_unknown_fields)]
pub struct OpenTab {
    pub tab: SearchEverywhereTab,
    #[serde(default)]
    pub query: String,
}

type TabOpener = Rc<dyn Fn(&mut Workspace, String, &mut Window, &mut Context<Workspace>)>;

#[derive(Default)]
struct SearchEverywhereRegistry {
    openers: HashMap<SearchEverywhereTab, TabOpener>,
    last_tab: Option<SearchEverywhereTab>,
}

impl Global for SearchEverywhereRegistry {}

/// Registers the picker that a Search Everywhere tab opens.
pub fn register_search_everywhere_tab(
    tab: SearchEverywhereTab,
    opener: impl Fn(&mut Workspace, String, &mut Window, &mut Context<Workspace>) + 'static,
    cx: &mut App,
) {
    cx.default_global::<SearchEverywhereRegistry>()
        .openers
        .insert(tab, Rc::new(opener));
}

fn registered_tabs(cx: &App) -> Vec<SearchEverywhereTab> {
    let Some(registry) = cx.try_global::<SearchEverywhereRegistry>() else {
        return Vec::new();
    };
    SearchEverywhereTab::ALL
        .into_iter()
        .filter(|tab| registry.openers.contains_key(tab))
        .collect()
}

/// The tab next to `tab`, wrapping around, among the registered ones.
pub fn adjacent_search_everywhere_tab(
    tab: SearchEverywhereTab,
    forward: bool,
    cx: &App,
) -> Option<SearchEverywhereTab> {
    let tabs = registered_tabs(cx);
    let index = tabs.iter().position(|candidate| *candidate == tab)?;
    let count = tabs.len();
    let next_index = if forward {
        (index + 1) % count
    } else {
        (index + count - 1) % count
    };
    tabs.get(next_index).copied()
}

pub(crate) fn open_tab(
    workspace: &mut Workspace,
    action: &OpenTab,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(opener) = cx
        .try_global::<SearchEverywhereRegistry>()
        .and_then(|registry| registry.openers.get(&action.tab).cloned())
    else {
        return;
    };
    if workspace.has_active_modal(window, cx) && !workspace.hide_modal(window, cx) {
        return;
    }
    cx.default_global::<SearchEverywhereRegistry>().last_tab = Some(action.tab);
    opener(workspace, action.query.clone(), window, cx);
}

pub(crate) fn toggle(
    workspace: &mut Workspace,
    _: &Toggle,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    if workspace.has_active_modal(window, cx) {
        workspace.hide_modal(window, cx);
        return;
    }
    let tab = cx
        .try_global::<SearchEverywhereRegistry>()
        .and_then(|registry| registry.last_tab)
        .unwrap_or(SearchEverywhereTab::Files);
    open_tab(
        workspace,
        &OpenTab {
            tab,
            query: String::new(),
        },
        window,
        cx,
    );
}

/// The row of tabs shown above a picker that is part of Search Everywhere.
pub fn render_search_everywhere_tabs(
    active_tab: SearchEverywhereTab,
    query: String,
    cx: &App,
) -> AnyElement {
    let tabs = registered_tabs(cx);
    if ui::islands_style(cx).enabled {
        return h_flex()
            .w_full()
            .px_2()
            .pt_2()
            .pb_1()
            .child(
                ui::islands_segmented_track(cx).children(tabs.into_iter().map(|tab| {
                    let query = query.clone();
                    ui::islands_segment(
                        SharedString::from(tab.label()),
                        tab.label(),
                        tab == active_tab,
                        cx,
                    )
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(
                            OpenTab {
                                tab,
                                query: query.clone(),
                            }
                            .boxed_clone(),
                            cx,
                        )
                    })
                })),
            )
            .child(div().flex_grow_1())
            .child(
                Label::new("Tab to switch")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .into_any_element();
    }
    h_flex()
        .w_full()
        .px_2()
        .pt_2()
        .pb_1()
        .gap_1()
        .children(tabs.into_iter().map(|tab| {
            let query = query.clone();
            Button::new(SharedString::from(tab.label()), tab.label())
                .size(ButtonSize::Compact)
                .style(ButtonStyle::Subtle)
                .label_size(LabelSize::Small)
                .selected_style(ButtonStyle::Filled)
                .selected_label_color(Color::Default)
                .toggle_state(tab == active_tab)
                .on_click(move |_, window, cx| {
                    window.dispatch_action(
                        OpenTab {
                            tab,
                            query: query.clone(),
                        }
                        .boxed_clone(),
                        cx,
                    )
                })
        }))
        .child(div().flex_grow_1())
        .child(
            Label::new("Tab to switch")
                .size(LabelSize::XSmall)
                .color(Color::Muted),
        )
        .into_any_element()
}
