use gpui::{App, Global, Pixels, px};

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
