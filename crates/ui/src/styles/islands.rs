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
