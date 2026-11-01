use gpui::{App, Div, FontWeight, Global, ParentElement, Pixels, Styled, div, px, rgb, white};

use crate::prelude::*;

/// How UI components should look when the workspace uses the islands layout,
/// where the editor area and docks are drawn as rounded panels.
///
/// The workspace keeps this in sync with its settings. Components read it with
/// [`islands_style`] and fall back to their regular look when it is disabled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IslandsStyle {
    pub enabled: bool,
    pub corner_radius: Pixels,
}

impl Default for IslandsStyle {
    fn default() -> Self {
        Self {
            enabled: false,
            corner_radius: px(10.),
        }
    }
}

impl Global for IslandsStyle {}

/// Returns the current islands style, or the disabled default if none is set.
pub fn islands_style(cx: &App) -> IslandsStyle {
    cx.try_global::<IslandsStyle>().copied().unwrap_or_default()
}

/// Corner radius for small controls inside an island, such as tabs and list rows.
pub fn islands_control_radius(cx: &App) -> Pixels {
    (islands_style(cx).corner_radius * 0.6).max(px(2.))
}

/// A rounded square with a project's initials on a color derived from its
/// name, like the project icons of JetBrains IDEs.
pub fn project_avatar(name: &str, cx: &App) -> Div {
    // Accent colours of the Islands title bar gradients.
    const COLORS: [u32; 9] = [
        0xB45E43, 0xA56906, 0x6B8E23, 0x2F7FB0, 0x5462C9, 0xA14D78, 0x8A5BAE, 0x2E8B7A, 0x4E9D6C,
    ];
    let words: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let initials: String = match words.as_slice() {
        [] => "?".to_string(),
        [word] => word.chars().take(2).collect(),
        [first, second, ..] => first
            .chars()
            .take(1)
            .chain(second.chars().take(1))
            .collect(),
    };
    let hash = name.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as u32)
    });
    let color = rgb(COLORS[hash as usize % COLORS.len()]);

    div()
        .flex_none()
        .size(px(18.))
        .rounded(px(4.))
        .bg(color)
        .flex()
        .items_center()
        .justify_center()
        .text_color(white())
        .text_size(px(9.))
        .font_weight(FontWeight::BOLD)
        .child(initials.to_uppercase())
        .when(cx.theme().appearance().is_light(), |this| this.opacity(0.9))
}

/// Symbol kinds that get one of IntelliJ's round symbol icons, as used by the
/// structure view and the completion popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IslandsSymbolKind {
    Class,
    Implementation,
    TypeAlias,
    Enum,
    Interface,
    Method,
    Function,
    Field,
    Property,
    Variable,
    Module,
}

impl IslandsSymbolKind {
    fn letter(self) -> &'static str {
        match self {
            Self::Class => "C",
            Self::Implementation => "i",
            Self::TypeAlias => "T",
            Self::Enum => "E",
            Self::Interface => "I",
            Self::Method => "m",
            Self::Function | Self::Field => "f",
            Self::Property => "p",
            Self::Variable => "v",
            Self::Module => "M",
        }
    }

    /// Foreground and background colors from IntelliJ's expui node icons.
    fn colors(self, is_light: bool) -> (u32, u32) {
        match (self, is_light) {
            (Self::Class | Self::Implementation | Self::TypeAlias, true) => (0x3574F0, 0xE7EFFD),
            (Self::Class | Self::Implementation | Self::TypeAlias, false) => (0x548AF7, 0x25324D),
            (Self::Enum | Self::Property, true) => (0x834DF0, 0xFAF5FF),
            (Self::Enum | Self::Property, false) => (0xA571E6, 0x2F2936),
            (Self::Interface, true) => (0x208A3C, 0xF2FCF3),
            (Self::Interface, false) => (0x57965C, 0x253627),
            (Self::Method | Self::Function, true) => (0xDB3B4B, 0xFFF7F7),
            (Self::Method | Self::Function, false) => (0xDB5C5C, 0x402929),
            (Self::Field | Self::Variable, true) => (0xE66D17, 0xFFF4EB),
            (Self::Field | Self::Variable, false) => (0xC77D55, 0x45322B),
            (Self::Module, true) => (0x6C707E, 0xEBECF0),
            (Self::Module, false) => (0xCED0D6, 0x43454A),
        }
    }
}

/// A round, letter-marked symbol icon like the ones in IntelliJ's structure view.
pub fn islands_symbol_badge(kind: IslandsSymbolKind, cx: &App) -> Div {
    let (foreground, background) = kind.colors(cx.theme().appearance().is_light());
    let foreground = rgb(foreground);
    div()
        .flex_none()
        .size(px(14.))
        .rounded_full()
        .border_1()
        .border_color(foreground)
        .bg(rgb(background))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(9.))
        .line_height(px(12.))
        .font_weight(FontWeight::BOLD)
        .text_color(foreground)
        .child(kind.letter())
}

/// The outlined track of an IntelliJ-style segmented control. Put
/// [`islands_segment`]s inside it.
pub fn islands_segmented_track(cx: &App) -> Div {
    let radius = islands_control_radius(cx);
    h_flex()
        .p_0p5()
        .gap_0p5()
        .rounded(radius + px(2.))
        .border_1()
        // Theme borders match popover backgrounds in the islands themes, so derive
        // the outline from the text color to keep it visible in both appearances.
        .border_color(cx.theme().colors().text.opacity(0.2))
}

/// One segment of an [`islands_segmented_track`]: the selected segment is filled,
/// the others highlight on hover.
pub fn islands_segment(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    cx: &App,
) -> gpui::Stateful<Div> {
    let colors = cx.theme().colors();
    let selected_background = colors.text.opacity(0.14);
    let hover_background = colors.ghost_element_hover;
    div()
        .id(id)
        .px_2()
        .py_px()
        .rounded(islands_control_radius(cx))
        .cursor_pointer()
        .map(|this| {
            if selected {
                this.bg(selected_background)
            } else {
                this.hover(move |style| style.bg(hover_background))
            }
        })
        .child(Label::new(label).color(if selected {
            Color::Default
        } else {
            Color::Muted
        }))
}
