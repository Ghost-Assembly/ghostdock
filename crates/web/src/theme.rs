//! Light or dark, per device.
//!
//! By default the page follows the device, through the stylesheet's
//! `prefers-color-scheme`. Choosing Light or Dark sets `data-theme` on the
//! root element, which the stylesheet lets win over the device. Nothing is
//! sent to the server: a phone in the sun and a desk at night are both the
//! same account.
//!
//! The script at the top of `index.html` does the rest, because it has to
//! run before the first paint anyway: it applies a stored choice before
//! anything is drawn, so a forced theme never flashes the other one, and it
//! watches `data-theme` to store each new choice and recolor the browser's
//! chrome. Kept there, the bundle carries none of it twice.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// Whatever the device prefers.
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    #[must_use]
    pub fn icon(self) -> &'static str {
        match self {
            Self::System => "monitor",
            Self::Light => "sun",
            Self::Dark => "moon",
        }
    }

    /// The value of `data-theme`; `None` for System, which sets nothing.
    fn key(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
        }
    }

    fn parse(set: Option<&str>) -> Self {
        match set {
            Some("light") => Self::Light,
            Some("dark") => Self::Dark,
            _ => Self::System,
        }
    }
}

fn root() -> Option<web_sys::Element> {
    web_sys::window()?.document()?.document_element()
}

/// The theme this device chose, as the page was started with it.
#[must_use]
pub fn current() -> Theme {
    Theme::parse(
        root()
            .and_then(|r| r.get_attribute("data-theme"))
            .as_deref(),
    )
}

/// Shows `theme` now; the page's script remembers it on this device.
pub fn choose(theme: Theme) {
    if let Some(root) = root() {
        let _ = match theme.key() {
            Some(key) => root.set_attribute("data-theme", key),
            None => root.remove_attribute("data-theme"),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anything_unrecognized_follows_the_device() {
        assert_eq!(Theme::parse(Some("light")), Theme::Light);
        assert_eq!(Theme::parse(Some("dark")), Theme::Dark);
        for other in [None, Some(""), Some("Dark"), Some("sepia")] {
            assert_eq!(Theme::parse(other), Theme::System, "{other:?}");
        }
        for theme in Theme::ALL {
            assert_eq!(Theme::parse(theme.key()), theme);
        }
    }
}
