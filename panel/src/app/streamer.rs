//! Streamer mode: what the screen may show of addresses, paths and text while it is on, and
//! turning it on and off.

use super::Panel;
use super::messages::ToastTone;
use crate::dashboard::LogEntry;
use crate::preferences;
use crate::privacy;
use crate::settings::{FieldKind, GROUPS};
use crate::widgets::dismiss_tooltips;
use gpui::Context;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;

impl Panel {
    /// `text` as the screen may show it: with IP addresses, the servers the player joins and
    /// the user's folder hidden in streamer mode.
    pub(super) fn shown_text(&self, text: &str) -> String {
        if self.preferences.streamer_mode {
            let text = privacy::hide_servers(text, self.known_servers());
            privacy::hide_in_text(&text, self.home.as_deref())
        } else {
            text.to_owned()
        }
    }

    /// The servers the player has joined, as typed: the recent ones, the one now joined,
    /// and every one joined since the panel opened, which lines still on screen may name.
    ///
    /// Each is where the server is, without the key an invite writes after it: the place is
    /// what must not be seen, with the key or alone.
    pub(super) fn known_servers(&self) -> impl Iterator<Item = &str> {
        self.preferences
            .servers
            .iter()
            .map(|server| server.address.as_str())
            .chain(std::iter::once(self.play.server()))
            .chain(self.play.servers_joined())
            .map(|server| preferences::server_parts(server).0)
            // The name the panel's own server is invited to by, which leads to this PC.
            .chain(self.preferences.invite_name.as_deref())
    }

    /// A log entry as one line on screen, hidden as streamer mode needs.
    pub(super) fn entry_text(&self, entry: &LogEntry) -> String {
        self.shown_text(&entry.text).replace('\n', " · ")
    }

    pub(super) fn shown_address(&self, address: SocketAddr) -> String {
        if self.preferences.streamer_mode {
            privacy::hide_address(address)
        } else {
            address.to_string()
        }
    }

    pub(super) fn shown_ip(&self, ip: IpAddr) -> String {
        if self.preferences.streamer_mode {
            privacy::hide_ip(ip)
        } else {
            ip.to_string()
        }
    }

    pub(super) fn shown_path(&self, path: &Path) -> String {
        self.shown_text(&path.display().to_string())
    }

    /// Turns streamer mode on or off, and remembers it.
    pub(super) fn set_streamer_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        self.preferences.streamer_mode = on;
        // A tooltip already open was written before, and could show what is now hidden.
        dismiss_tooltips(cx);
        self.mask_fields(cx);
        self.save_preferences();
        self.show_toast(
            ToastTone::Success,
            if on {
                "Streamer mode on: IP addresses, server names and your user folder are hidden"
                    .to_owned()
            } else {
                "Streamer mode off".to_owned()
            },
        );
        cx.notify();
    }

    /// Shows the fields that can hold an address or a path as dots in streamer mode: the
    /// listen address, the name for invites, the address to ban, the Play page's address,
    /// and paths among the settings. The password is
    /// always dots in streamer mode, and goes back to dots when it ends.
    pub(super) fn mask_fields(&mut self, cx: &mut Context<Self>) {
        let on = self.preferences.streamer_mode;
        if on {
            self.play.hide_password(cx);
        }
        let addresses = [
            &self.address,
            &self.invite_name,
            &self.ban_input,
            self.play.address_input(),
        ];
        for input in addresses {
            input.update(cx, |input, cx| input.set_masked(on, cx));
        }
        for (key, input) in &self.fields {
            let kind = GROUPS
                .iter()
                .flat_map(|group| group.fields)
                .find(|field| field.key == *key)
                .map(|field| field.kind);
            match kind {
                Some(FieldKind::Path) => input.update(cx, |input, cx| input.set_masked(on, cx)),
                Some(FieldKind::Password) => {
                    input.update(cx, |input, cx| input.set_masked(true, cx));
                }
                _ => {}
            }
        }
    }
}
