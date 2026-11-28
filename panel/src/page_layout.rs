//! The order of each page's panels, which the user chooses and the preferences keep. Every
//! panel of a page is always there: only their order changes. The Overview has its own, fuller
//! layout (`overview_layout`), and the Log is one list.

/// A page whose panels can be put in another order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrangedPage {
    Play,
    Players,
    Bans,
    Settings,
    Bots,
    Preferences,
}

/// A panel of a page: how the preferences save it, and what the strip that moves it calls it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PagePanel {
    pub key: &'static str,
    pub title: &'static str,
}

const fn panel(key: &'static str, title: &'static str) -> PagePanel {
    PagePanel { key, title }
}

impl ArrangedPage {
    pub const ALL: [Self; 6] = [
        Self::Play,
        Self::Players,
        Self::Bans,
        Self::Settings,
        Self::Bots,
        Self::Preferences,
    ];

    /// The preference that holds this page's order.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Play => "play_panels",
            Self::Players => "players_panels",
            Self::Bans => "bans_panels",
            Self::Settings => "settings_panels",
            Self::Bots => "bots_panels",
            Self::Preferences => "preferences_panels",
        }
    }

    /// The page's panels, in the order it has until the user changes it. A page may leave a
    /// panel out for now, as the Play page does its chat while not in a session.
    #[must_use]
    pub fn panels(self) -> &'static [PagePanel] {
        const PLAY: &[PagePanel] = &[
            panel("status", "Session"),
            panel("chat", "Chat"),
            panel("server", "Server"),
            panel("game", "Game"),
        ];
        const PLAYERS: &[PagePanel] = &[panel("chat", "Chat"), panel("table", "Players")];
        const BANS: &[PagePanel] = &[
            panel("add", "Ban an address"),
            panel("list", "Banned addresses"),
        ];
        // After the recent files, the groups of `settings::GROUPS` by their titles in
        // lowercase, which a test checks.
        const SETTINGS: &[PagePanel] = &[
            panel("recent", "Recent files"),
            panel("session", "Session"),
            panel("combat", "Combat"),
            panel("movement checks", "Movement checks"),
            panel("rate limits", "Rate limits"),
            panel("bans", "Bans"),
            panel("advanced", "Advanced"),
        ];
        const PREFERENCES: &[PagePanel] = &[
            panel("appearance", "Appearance"),
            panel("privacy", "Privacy"),
            panel("behavior", "Behavior"),
            panel("chat", "Chat in the game"),
            panel("alerts", "Alerts"),
            panel("startup", "Startup"),
            panel("shortcuts", "Keyboard shortcuts"),
            panel("about", "About"),
            panel("reset", "Reset"),
        ];
        const BOTS: &[PagePanel] = &[panel("yours", "Your game"), panel("games", "Bot games")];
        match self {
            Self::Play => PLAY,
            Self::Bots => BOTS,
            Self::Players => PLAYERS,
            Self::Bans => BANS,
            Self::Settings => SETTINGS,
            Self::Preferences => PREFERENCES,
        }
    }
}

/// One page's panels in the user's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageLayout {
    order: Vec<PagePanel>,
}

impl PageLayout {
    fn usual(page: ArrangedPage) -> Self {
        Self {
            order: page.panels().to_vec(),
        }
    }

    /// Reads an order as `format` writes it, as `table,chat`. Panels named twice or not known
    /// are left out, and every panel is there: one the order does not name, as a panel added
    /// to the page since the order was saved, goes after the panel it usually follows,
    /// wherever the user has that one, and so not after a panel that is usually last.
    fn parse(page: ArrangedPage, text: &str) -> Self {
        let usual = page.panels();
        let mut order: Vec<PagePanel> = Vec::new();
        let named = text
            .split(',')
            .filter_map(|key| usual.iter().find(|panel| panel.key == key.trim()));
        for panel in named {
            if !order.contains(panel) {
                order.push(*panel);
            }
        }
        let mut before: Option<&PagePanel> = None;
        for panel in usual {
            if !order.contains(panel) {
                let followed = before.and_then(|before| order.iter().position(|p| p == before));
                order.insert(followed.map_or(0, |at| at + 1), *panel);
            }
            before = Some(panel);
        }
        Self { order }
    }

    #[must_use]
    pub fn format(&self) -> String {
        let keys: Vec<&str> = self.order.iter().map(|panel| panel.key).collect();
        keys.join(",")
    }

    #[must_use]
    pub fn order(&self) -> &[PagePanel] {
        &self.order
    }

    /// Where the panel called `key` would go if moved one place earlier, or later, among the
    /// panels `shown`: the place of its neighbor there. The panels a page leaves out for now
    /// are passed over, so that a move always shows.
    fn destination(&self, key: &str, later: bool, shown: &[&str]) -> Option<(usize, usize)> {
        let from = self.order.iter().position(|panel| panel.key == key)?;
        let mut places = self.order.iter().enumerate();
        let is_shown = |(_, panel): &(usize, &PagePanel)| shown.contains(&panel.key);
        let to = if later {
            places.skip(from + 1).find(is_shown)
        } else {
            places.by_ref().take(from).filter(is_shown).last()
        };
        to.map(|(to, _)| (from, to))
    }

    /// Whether the panel called `key` can go one place earlier, or later, among `shown`.
    #[must_use]
    pub fn can_move(&self, key: &str, later: bool, shown: &[&str]) -> bool {
        self.destination(key, later, shown).is_some()
    }

    /// Moves the panel called `key` past its neighbor among `shown`, if it has one that way.
    pub fn nudge(&mut self, key: &str, later: bool, shown: &[&str]) {
        if let Some((from, to)) = self.destination(key, later, shown) {
            let panel = self.order.remove(from);
            self.order.insert(to, panel);
        }
    }
}

/// Every arranged page's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageLayouts {
    play: PageLayout,
    players: PageLayout,
    bans: PageLayout,
    settings: PageLayout,
    bots: PageLayout,
    preferences: PageLayout,
}

impl Default for PageLayouts {
    fn default() -> Self {
        Self {
            play: PageLayout::usual(ArrangedPage::Play),
            players: PageLayout::usual(ArrangedPage::Players),
            bans: PageLayout::usual(ArrangedPage::Bans),
            settings: PageLayout::usual(ArrangedPage::Settings),
            bots: PageLayout::usual(ArrangedPage::Bots),
            preferences: PageLayout::usual(ArrangedPage::Preferences),
        }
    }
}

impl PageLayouts {
    #[must_use]
    pub fn of(&self, page: ArrangedPage) -> &PageLayout {
        match page {
            ArrangedPage::Play => &self.play,
            ArrangedPage::Players => &self.players,
            ArrangedPage::Bans => &self.bans,
            ArrangedPage::Settings => &self.settings,
            ArrangedPage::Bots => &self.bots,
            ArrangedPage::Preferences => &self.preferences,
        }
    }

    pub fn of_mut(&mut self, page: ArrangedPage) -> &mut PageLayout {
        match page {
            ArrangedPage::Play => &mut self.play,
            ArrangedPage::Players => &mut self.players,
            ArrangedPage::Bans => &mut self.bans,
            ArrangedPage::Settings => &mut self.settings,
            ArrangedPage::Bots => &mut self.bots,
            ArrangedPage::Preferences => &mut self.preferences,
        }
    }

    /// Takes a line of the preferences, if `key` is a page's; says whether it was.
    pub fn read(&mut self, key: &str, value: &str) -> bool {
        let page = ArrangedPage::ALL.into_iter().find(|page| page.key() == key);
        if let Some(page) = page {
            *self.of_mut(page) = PageLayout::parse(page, value);
        }
        page.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::{ArrangedPage, PageLayout, PageLayouts};
    use crate::settings::GROUPS;

    fn keys(layout: &PageLayout) -> Vec<&'static str> {
        layout.order().iter().map(|panel| panel.key).collect()
    }

    #[test]
    fn an_order_reads_back_and_always_holds_every_panel() {
        let page = ArrangedPage::Players;
        let mut layout = PageLayout::usual(page);
        layout.nudge("chat", true, &["chat", "table"]);
        assert_eq!(keys(&layout), ["table", "chat"]);
        assert_eq!(PageLayout::parse(page, &layout.format()), layout);

        // Nonsense, a panel twice, and a panel missing all still give both, once each.
        for saved in ["", "nonsense", "table,table", "table"] {
            let order = PageLayout::parse(page, saved);
            assert_eq!(order.order().len(), page.panels().len(), "{saved}");
        }
        assert_eq!(keys(&PageLayout::parse(page, "table")), ["chat", "table"]);
    }

    #[test]
    fn a_panel_added_since_an_order_was_saved_takes_its_usual_place() {
        let page = ArrangedPage::Preferences;
        // Saved before the page had its chat panel, which usually follows Behavior, and its
        // About, which usually follows the shortcuts.
        let saved = "appearance,privacy,behavior,alerts,startup,shortcuts,reset";
        assert_eq!(PageLayout::parse(page, saved), PageLayout::usual(page));
        // The user's own order is kept, and each new panel still follows its neighbor there.
        let moved = "shortcuts,behavior,appearance,privacy,alerts,startup,reset";
        assert_eq!(
            keys(&PageLayout::parse(page, moved)),
            [
                "shortcuts",
                "about",
                "behavior",
                "chat",
                "appearance",
                "privacy",
                "alerts",
                "startup",
                "reset"
            ]
        );
    }

    #[test]
    fn resetting_is_the_last_panel_of_the_preferences_until_moved() {
        let last = ArrangedPage::Preferences.panels().last();
        assert_eq!(last.map(|panel| panel.key), Some("reset"));
    }

    #[test]
    fn a_panel_at_an_end_cannot_move_past_it() {
        let shown = ["chat", "table"];
        let mut layout = PageLayout::usual(ArrangedPage::Players);
        assert!(!layout.can_move("chat", false, &shown));
        assert!(layout.can_move("chat", true, &shown));
        layout.nudge("chat", false, &shown);
        layout.nudge("table", true, &shown);
        assert_eq!(layout, PageLayout::usual(ArrangedPage::Players));
    }

    #[test]
    fn a_move_passes_over_panels_the_page_leaves_out() {
        // The Play page with no chat, as when not in a session.
        let shown = ["status", "server", "game"];
        let mut layout = PageLayout::usual(ArrangedPage::Play);
        layout.nudge("server", false, &shown);
        assert_eq!(keys(&layout), ["server", "status", "chat", "game"]);
        assert!(!layout.can_move("server", false, &shown));
        layout.nudge("status", true, &shown);
        assert_eq!(keys(&layout), ["server", "chat", "game", "status"]);
        assert!(!layout.can_move("status", true, &shown));
    }

    #[test]
    fn each_page_keeps_its_own_order() {
        let mut layouts = PageLayouts::default();
        assert!(layouts.read("bans_panels", "list,add"));
        assert!(!layouts.read("overview", "list,add"));
        assert_eq!(keys(layouts.of(ArrangedPage::Bans)), ["list", "add"]);
        assert_eq!(
            layouts.of(ArrangedPage::Players),
            PageLayouts::default().of(ArrangedPage::Players)
        );
    }

    #[test]
    fn the_settings_page_has_a_panel_for_every_group_of_settings() {
        let panels = ArrangedPage::Settings.panels();
        for group in GROUPS {
            let key = group.title.to_lowercase();
            let panel = panels.iter().find(|panel| panel.key == key);
            assert_eq!(panel.map(|panel| panel.title), Some(group.title));
        }
        assert_eq!(panels.len(), GROUPS.len() + 1);
    }

    #[test]
    fn no_page_names_a_panel_twice() {
        for page in ArrangedPage::ALL {
            let mut keys: Vec<&str> = page.panels().iter().map(|panel| panel.key).collect();
            keys.sort_unstable();
            keys.dedup();
            assert_eq!(keys.len(), page.panels().len(), "{page:?}");
        }
    }
}
