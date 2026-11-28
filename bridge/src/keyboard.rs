//! The PC's keyboard, as the chat reads it while the game's window is in front.
//!
//! The game cannot read a keyboard: the emulator gives it only the keys bound to its
//! controller. So the bridge reads the keyboard for it, and as little of it as will do: only
//! while the emulator's window has the keyboard, only the keys that open the chat and the one
//! that shows or hides the minimap until the chat opens, and the typing keys only while a line
//! is open. It asks Windows which of those keys are
//! down each time round its loop. It installs no hook, so no key pressed in another program
//! ever passes through here.
//!
//! Reading a key does not take it from the emulator: the emulator still acts on a hotkey
//! bound to it (the panel turns those on typing keys off in a game it starts) and still gives
//! the game a key bound to its controller, which the game ignores while a line is open
//! (`mp_read_pads` in `TrollEngine/Multiplayer/people.c`).

use crate::typing::Key;
use std::time::{Duration, Instant};

/// How long a key is held before it repeats, and how often it does from then on.
const REPEAT_DELAY: Duration = Duration::from_millis(450);
const REPEAT_INTERVAL: Duration = Duration::from_millis(40);
/// The longest the keys still held as the chat closes are waited on.
const SETTLE_LIMIT: Duration = Duration::from_secs(2);

/// Windows' codes for the keys that edit a line; `Keys::typed` says what any key types.
pub const BACKSPACE: u16 = 0x08;
pub const ENTER: u16 = 0x0D;
pub const ESCAPE: u16 = 0x1B;
const SPACE: u16 = 0x20;
/// The keys that scroll the open chat, and bring back the lines sent before.
pub const PAGE_UP: u16 = 0x21;
pub const PAGE_DOWN: u16 = 0x22;
pub const ARROW_UP: u16 = 0x26;
pub const ARROW_DOWN: u16 = 0x28;
/// One more than the highest key code.
const KEY_CODES: usize = 256;

/// The keys watched while a line is open: those that edit it or scroll the chat, and every
/// key that types a character on one keyboard layout or another. By Windows' codes, these are
/// the space, the digits, the letters, the number pad, and the keys for marks, which differ by
/// layout.
fn typing_keys() -> impl Iterator<Item = u16> {
    [
        BACKSPACE, ENTER, ESCAPE, SPACE, PAGE_UP, PAGE_DOWN, ARROW_UP, ARROW_DOWN,
    ]
    .into_iter()
    .chain(0x30..=0x39)
    .chain(0x41..=0x5A)
    .chain(0x60..=0x6F)
    .chain(0xBA..=0xC0)
    .chain(0xDB..=0xDF)
    .chain([0xE2])
}

/// The keyboard as the system reports it. Apart from tests, this is `WindowsKeys`.
pub trait Keys {
    /// Whether the game's window has the keyboard.
    fn has_focus(&self) -> bool;
    /// Whether `key` is down, or was pressed and let go since it was last asked about.
    fn is_down(&self, key: u16) -> bool;
    /// What pressing `key` does to a line now, with Shift, Caps Lock and the like as they are:
    /// a character, or one of the keys that edit. Nothing for a key that types nothing, or
    /// one pressed as a shortcut.
    fn typed(&self, key: u16) -> Option<Key>;
    /// The key that types `character`, alone or with Shift, if the keyboard has one.
    fn key_for(&self, character: char) -> Option<u16>;
}

/// Turns the keys' states into the keys pressed for the chat, and for the minimap.
#[derive(Debug)]
pub struct Keyboard<K> {
    keys: K,
    /// The character whose key opens the chat, if the player types in the game.
    opener: Option<char>,
    /// The character whose key shows or hides the minimap, if any does.
    map_key: Option<char>,
    /// Whether each key was down when it was last looked at.
    down: [bool; KEY_CODES],
    /// Whether a line was open at the last poll, if the window had the keyboard then.
    watching: Option<bool>,
    /// The key being held that repeats, and when it next does.
    repeat: Option<(u16, Instant)>,
    /// The keys that were down when the chat closed and have not been let go since, and when
    /// to stop waiting for them.
    settling: Vec<u16>,
    settle_until: Option<Instant>,
}

impl<K: Keys> Keyboard<K> {
    /// A reader of `keys` for a chat that the key typing `opener` opens, or none for a player
    /// who does not type in the game, and the slash then opens nothing either.
    pub const fn new(keys: K, opener: Option<char>) -> Self {
        Self {
            keys,
            opener,
            map_key: None,
            down: [false; KEY_CODES],
            watching: None,
            repeat: None,
            settling: Vec::new(),
            settle_until: None,
        }
    }

    /// Has the key typing `key` show and hide the minimap while the chat is closed. The chat's
    /// key comes first, should the two be the same.
    #[must_use]
    pub const fn with_map_key(mut self, key: Option<char>) -> Self {
        self.map_key = key;
        self
    }

    /// Has the next `poll` take nothing for a press: after a while in which the keys were
    /// not looked at, as while joining a server again, a key pressed then was for something
    /// else, and Windows still reports it as pressed since the last look.
    pub fn look_afresh(&mut self) {
        self.watching = None;
        self.repeat = None;
    }

    /// The keys pressed for the chat since the last poll, in the order of their codes; `open`
    /// says whether a line is being typed.
    ///
    /// While none is, only the keys that open the chat are looked at. A key held down types
    /// again and again, as it does anywhere. A key that was already down when the chat opened
    /// or closed, or when the window came to the front, was pressed for something else, and
    /// types nothing until it is pressed again.
    pub fn poll(&mut self, open: bool, now: Instant) -> Vec<Key> {
        if !self.keys.has_focus() {
            // What is typed into another window is not the chat's to see, or to wait on.
            self.watching = None;
            self.repeat = None;
            self.settling.clear();
            return Vec::new();
        }
        let was_watching = self.watching.replace(open);
        let is_same_watch = was_watching == Some(open);
        if was_watching == Some(true) && !open {
            self.settling = typing_keys()
                .filter(|&key| self.keys.is_down(key))
                .collect();
            self.settle_until = Some(now + SETTLE_LIMIT);
        }
        self.settling.retain(|&key| self.keys.is_down(key));

        let watched: Vec<u16> = if open {
            typing_keys().collect()
        } else {
            self.openers()
        };
        let mut pressed = Vec::new();
        for key in watched {
            let is_down = self.keys.is_down(key);
            let Some(was_down) = self.down.get_mut(usize::from(key)) else {
                continue;
            };
            let was_down = std::mem::replace(was_down, is_down);
            if is_down
                && !was_down
                && is_same_watch
                && let Some(typed) = self.press(key, open)
            {
                pressed.push(typed);
                self.repeat = Some((key, now + REPEAT_DELAY));
            }
        }
        pressed.extend(self.repeated(open, now));
        pressed
    }

    /// Whether the game's window had the keyboard at the last poll. A line cannot be typed
    /// while it has not, so one that is open then is best given up.
    #[must_use]
    pub const fn has_keyboard(&self) -> bool {
        self.watching.is_some()
    }

    /// Whether a key held as the chat closed is still down, so the game is not yet to see the
    /// keys: the Enter that sent a line is also the game's Start button, on many a keyboard.
    #[must_use]
    pub fn is_settling(&self, now: Instant) -> bool {
        !self.settling.is_empty() && self.settle_until.is_some_and(|until| now < until)
    }

    /// The keys watched while the chat is closed: the opener's and the slash's, which open it,
    /// and the minimap's.
    fn openers(&self) -> Vec<u16> {
        let slash = self.opener.map(|_| '/');
        let mut keys: Vec<u16> = [self.opener, slash, self.map_key]
            .into_iter()
            .flatten()
            .filter_map(|character| self.keys.key_for(character))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    /// What `key`, just pressed, is to the chat.
    fn press(&self, key: u16, open: bool) -> Option<Key> {
        let typed = self.keys.typed(key)?;
        if open {
            return Some(typed);
        }
        let Key::Character(character) = typed else {
            return None;
        };
        let is =
            |named: Option<char>| named.is_some_and(|named| named.eq_ignore_ascii_case(&character));
        if is(self.opener) {
            Some(Key::Chat)
        } else if character == '/' && self.opener.is_some() {
            Some(Key::Command)
        } else if is(self.map_key) {
            Some(Key::Map)
        } else {
            None
        }
    }

    /// The held key's repeat, if it is due; only a line being typed has one.
    fn repeated(&mut self, open: bool, now: Instant) -> Option<Key> {
        let (key, due) = self.repeat?;
        let is_held = self.down.get(usize::from(key)).is_some_and(|&down| down);
        if !open || !is_held {
            self.repeat = None;
            return None;
        }
        if now < due {
            return None;
        }
        self.repeat = Some((key, now + REPEAT_INTERVAL));
        self.keys.typed(key)
    }
}

#[cfg(windows)]
pub use system::WindowsKeys;

#[cfg(windows)]
mod system {
    use super::{
        ARROW_DOWN, ARROW_UP, BACKSPACE, ENTER, ESCAPE, KEY_CODES, Keys, PAGE_DOWN, PAGE_UP,
    };
    use crate::typing::Key;
    use std::mem::MaybeUninit;
    use std::ptr;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, GetKeyState, GetKeyboardLayout, HKL, MAPVK_VK_TO_VSC, MapVirtualKeyExW,
        ToUnicodeEx, VK_CAPITAL, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VkKeyScanExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, MSG, PM_NOREMOVE, PeekMessageW,
    };

    /// In a key's state as `ToUnicodeEx` takes it: the key is down, or, for a lock key, on.
    const DOWN: u8 = 0x80;
    const LOCKED: u8 = 0x01;
    /// Has `ToUnicodeEx` leave the keyboard's own state alone, which it otherwise changes for
    /// keys that put an accent on the next. Windows before 10 ignores it.
    const KEEP_STATE: u32 = 0x4;

    /// The keyboard of this PC, for the chat of the game in one emulator.
    #[derive(Debug)]
    pub struct WindowsKeys {
        /// The emulator's process: only its windows' keys are read.
        pid: u32,
    }

    impl WindowsKeys {
        #[must_use]
        pub const fn new(pid: u32) -> Self {
            Self { pid }
        }

        /// The thread of the window that has the keyboard, if the window is the emulator's.
        fn focused_thread(&self) -> Option<u32> {
            // SAFETY: takes no pointers.
            let window = unsafe { GetForegroundWindow() };
            let mut pid = 0;
            // SAFETY: `pid` is valid for one write. A window that is null, or has closed since
            // it was found, is reported as no thread.
            let thread = unsafe { GetWindowThreadProcessId(window, &raw mut pid) };
            (thread != 0 && pid == self.pid).then_some(thread)
        }

        /// The keyboard layout the emulator's window types with.
        fn layout(&self) -> Option<HKL> {
            let thread = self.focused_thread()?;
            // SAFETY: takes no pointers.
            Some(unsafe { GetKeyboardLayout(thread) })
        }

        /// The character `key` types now, if it types one the game can draw.
        fn character(&self, key: u16) -> Option<char> {
            let layout = self.layout()?;
            let mut state = [0_u8; KEY_CODES];
            for modifier in [VK_SHIFT, VK_CONTROL, VK_MENU] {
                if is_held(modifier)
                    && let Some(held) = state.get_mut(usize::from(modifier))
                {
                    *held = DOWN;
                }
            }
            if is_caps_lock_on()
                && let Some(lock) = state.get_mut(usize::from(VK_CAPITAL))
            {
                *lock = LOCKED;
            }
            // SAFETY: takes no pointers.
            let scan = unsafe { MapVirtualKeyExW(u32::from(key), MAPVK_VK_TO_VSC, layout) };
            let mut typed = [0_u16; 4];
            // SAFETY: `state` is the 256 bytes the call reads, and `typed` is valid for the 4
            // units it is told it may write.
            let count = unsafe {
                ToUnicodeEx(
                    u32::from(key),
                    scan,
                    state.as_ptr(),
                    typed.as_mut_ptr(),
                    4,
                    KEEP_STATE,
                    layout,
                )
            };
            // One unit: neither nothing, nor an accent waiting for its letter.
            let unit = typed.first().filter(|_| count == 1)?;
            char::from_u32(u32::from(*unit))
        }
    }

    /// Whether `key` is down now.
    fn is_held(key: u16) -> bool {
        // SAFETY: takes no pointers.
        let state = unsafe { GetAsyncKeyState(i32::from(key)) };
        state < 0
    }

    /// Whether Caps Lock is on. Windows keeps that for each thread, as of the last key message
    /// the thread took, and this thread has no window to be sent any: asking marks its record
    /// as due to be brought up to date, looking for a message does so, and then it is right.
    fn is_caps_lock_on() -> bool {
        // SAFETY: takes no pointers.
        unsafe { GetKeyState(i32::from(VK_CAPITAL)) };
        let mut message = MaybeUninit::<MSG>::uninit();
        // SAFETY: `message` is valid for one write, and is not read: there is none to take,
        // and with PM_NOREMOVE one would be left where it is.
        unsafe { PeekMessageW(message.as_mut_ptr(), ptr::null_mut(), 0, 0, PM_NOREMOVE) };
        // SAFETY: takes no pointers.
        let state = unsafe { GetKeyState(i32::from(VK_CAPITAL)) };
        state & 1 != 0
    }

    impl Keys for WindowsKeys {
        fn has_focus(&self) -> bool {
            self.focused_thread().is_some()
        }

        fn is_down(&self, key: u16) -> bool {
            // SAFETY: takes no pointers.
            let state = unsafe { GetAsyncKeyState(i32::from(key)) };
            // The low bit is a press since the last call, which catches a key tapped and let
            // go between two polls. Another program asking clears it, so it is only a help.
            state < 0 || state & 1 != 0
        }

        fn typed(&self, key: u16) -> Option<Key> {
            // With Ctrl, Alt or the Windows key, a key is a shortcut: the emulator's, as
            // Alt+Enter is, or Windows' own. Ctrl and Alt together are AltGr, which types.
            if is_held(VK_CONTROL) != is_held(VK_MENU) || is_held(VK_LWIN) || is_held(VK_RWIN) {
                return None;
            }
            match key {
                BACKSPACE => Some(Key::Backspace),
                ENTER => Some(Key::Enter),
                ESCAPE => Some(Key::Escape),
                PAGE_UP => Some(Key::PageUp),
                PAGE_DOWN => Some(Key::PageDown),
                ARROW_UP => Some(Key::Up),
                ARROW_DOWN => Some(Key::Down),
                _ => self.character(key).map(Key::Character),
            }
        }

        fn key_for(&self, character: char) -> Option<u16> {
            let layout = self.layout()?;
            let unit = u16::try_from(u32::from(character)).ok()?;
            // SAFETY: takes no pointers.
            let found = unsafe { VkKeyScanExW(unit, layout) };
            // The key's code in the low byte, and the Shift it needs in the high one, which
            // `typed` sees for itself; or -1 if no key types it.
            let [key, _] = found.to_le_bytes();
            (found != -1).then_some(u16::from(key))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BACKSPACE, ENTER, Keyboard, Keys, REPEAT_DELAY, REPEAT_INTERVAL, SETTLE_LIMIT};
    use crate::typing::Key;
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeSet;
    use std::time::Instant;

    const T: u16 = 0x54;
    const W: u16 = 0x57;
    const SEVEN: u16 = 0x37;
    const SLASH: u16 = 0xBF;
    const SHIFT: u16 = 0x10;

    /// A keyboard a test presses the keys of: letters and digits type themselves, and the
    /// slash is on its own key, or on Shift and 7 as German keyboards have it.
    #[derive(Default)]
    struct FakeKeys {
        unfocused: Cell<bool>,
        down: RefCell<BTreeSet<u16>>,
        german: bool,
    }

    impl FakeKeys {
        fn hold(&self, key: u16) {
            self.down.borrow_mut().insert(key);
        }

        fn release(&self, key: u16) {
            self.down.borrow_mut().remove(&key);
        }
    }

    impl Keys for &FakeKeys {
        fn has_focus(&self) -> bool {
            !self.unfocused.get()
        }

        fn is_down(&self, key: u16) -> bool {
            self.down.borrow().contains(&key)
        }

        fn typed(&self, key: u16) -> Option<Key> {
            let shifted = self.is_down(SHIFT);
            match key {
                BACKSPACE => Some(Key::Backspace),
                ENTER => Some(Key::Enter),
                SEVEN if self.german && shifted => Some(Key::Character('/')),
                SLASH if !self.german => Some(Key::Character('/')),
                0x30..=0x39 | 0x41..=0x5A => {
                    let character = char::from(u8::try_from(key).ok()?);
                    Some(Key::Character(if shifted {
                        character
                    } else {
                        character.to_ascii_lowercase()
                    }))
                }
                _ => None,
            }
        }

        fn key_for(&self, character: char) -> Option<u16> {
            match character {
                '/' if self.german => Some(SEVEN),
                '/' => Some(SLASH),
                _ => u16::try_from(u32::from(character.to_ascii_uppercase())).ok(),
            }
        }
    }

    #[test]
    fn only_the_opening_keys_are_read_until_the_chat_opens() {
        let keys = FakeKeys::default();
        let mut keyboard = Keyboard::new(&keys, Some('t'));
        let now = Instant::now();
        assert_eq!(keyboard.poll(false, now), []);

        // Played with, not typed: no key but the chat's own says anything.
        keys.hold(W);
        assert_eq!(keyboard.poll(false, now), []);
        keys.hold(T);
        assert_eq!(keyboard.poll(false, now), [Key::Chat]);
        // Held, it opens the chat once.
        assert_eq!(keyboard.poll(false, now), []);
        keys.release(T);
        keys.hold(SLASH);
        assert_eq!(keyboard.poll(false, now), [Key::Command]);
    }

    #[test]
    fn the_map_key_toggles_while_the_chat_is_closed_and_types_while_it_is_open() {
        const M: u16 = 0x4D;
        let keys = FakeKeys::default();
        let mut keyboard = Keyboard::new(&keys, Some('t')).with_map_key(Some('m'));
        let now = Instant::now();
        assert_eq!(keyboard.poll(false, now), []);
        keys.hold(M);
        assert_eq!(keyboard.poll(false, now), [Key::Map]);
        // Held, it does not repeat.
        assert_eq!(keyboard.poll(false, now), []);
        keys.release(M);
        assert_eq!(keyboard.poll(true, now), []);
        keys.hold(M);
        assert_eq!(keyboard.poll(true, now), [Key::Character('m')]);

        // With the chat turned off, the map key is still read, and neither T nor the slash
        // opens anything.
        let keys = FakeKeys::default();
        let mut keyboard = Keyboard::new(&keys, None).with_map_key(Some('m'));
        assert_eq!(keyboard.poll(false, now), []);
        for key in [T, SLASH, M] {
            keys.hold(key);
        }
        assert_eq!(keyboard.poll(false, now), [Key::Map]);
    }

    #[test]
    fn a_key_pressed_while_the_keys_were_not_looked_at_opens_nothing() {
        let keys = FakeKeys::default();
        let mut keyboard = Keyboard::new(&keys, Some('t'));
        let now = Instant::now();
        assert_eq!(keyboard.poll(false, now), []);

        // Pressed while the bridge was joining its server again, and still seen as pressed.
        keys.hold(T);
        keyboard.look_afresh();
        assert_eq!(keyboard.poll(false, now), []);
        // Pressed again, it is the player's.
        keys.release(T);
        assert_eq!(keyboard.poll(false, now), []);
        keys.hold(T);
        assert_eq!(keyboard.poll(false, now), [Key::Chat]);
    }

    #[test]
    fn the_slash_opens_the_chat_wherever_the_keyboard_has_it() {
        let keys = FakeKeys {
            german: true,
            ..FakeKeys::default()
        };
        let mut keyboard = Keyboard::new(&keys, Some('t'));
        let now = Instant::now();
        keyboard.poll(false, now);

        // A 7 is a 7, and with Shift it is the slash.
        keys.hold(SEVEN);
        assert_eq!(keyboard.poll(false, now), []);
        keys.release(SEVEN);
        keyboard.poll(false, now);
        keys.hold(SHIFT);
        keys.hold(SEVEN);
        assert_eq!(keyboard.poll(false, now), [Key::Command]);
    }

    #[test]
    fn keys_held_as_the_chat_opens_type_nothing_until_pressed_again() {
        let keys = FakeKeys::default();
        let mut keyboard = Keyboard::new(&keys, Some('t'));
        let now = Instant::now();
        keyboard.poll(false, now);
        // Running forward, the player opens the chat.
        keys.hold(W);
        keys.hold(T);
        assert_eq!(keyboard.poll(false, now), [Key::Chat]);

        assert_eq!(keyboard.poll(true, now), []);
        // Neither repeats, for all that both stay down.
        assert_eq!(keyboard.poll(true, now + REPEAT_DELAY * 2), []);
        keys.release(W);
        keys.release(T);
        keyboard.poll(true, now);
        keys.hold(W);
        assert_eq!(keyboard.poll(true, now), [Key::Character('w')]);
        keys.hold(T);
        assert_eq!(keyboard.poll(true, now), [Key::Character('t')]);
    }

    #[test]
    fn a_held_key_repeats_after_a_pause_and_stops_when_let_go() {
        let keys = FakeKeys::default();
        let mut keyboard = Keyboard::new(&keys, Some('t'));
        let start = Instant::now();
        keyboard.poll(true, start);

        keys.hold(BACKSPACE);
        assert_eq!(keyboard.poll(true, start), [Key::Backspace]);
        assert_eq!(keyboard.poll(true, start + REPEAT_DELAY / 2), []);
        let first = start + REPEAT_DELAY;
        assert_eq!(keyboard.poll(true, first), [Key::Backspace]);
        assert_eq!(keyboard.poll(true, first), []);
        assert_eq!(
            keyboard.poll(true, first + REPEAT_INTERVAL),
            [Key::Backspace]
        );

        keys.release(BACKSPACE);
        assert_eq!(keyboard.poll(true, first + REPEAT_INTERVAL * 5), []);
    }

    #[test]
    fn nothing_is_read_while_another_window_has_the_keyboard() {
        let keys = FakeKeys::default();
        let mut keyboard = Keyboard::new(&keys, Some('t'));
        let now = Instant::now();
        keyboard.poll(true, now);

        assert!(keyboard.has_keyboard());

        // Typed into another program.
        keys.unfocused.set(true);
        keys.hold(W);
        assert_eq!(keyboard.poll(true, now), []);
        assert!(!keyboard.has_keyboard());
        // Still down when the game's window comes back: not typed here either.
        keys.unfocused.set(false);
        assert_eq!(keyboard.poll(true, now), []);
        keys.release(W);
        keyboard.poll(true, now);
        keys.hold(W);
        assert_eq!(keyboard.poll(true, now), [Key::Character('w')]);
    }

    #[test]
    fn the_keys_held_as_the_chat_closes_are_waited_on_and_no_others() {
        let keys = FakeKeys::default();
        let mut keyboard = Keyboard::new(&keys, Some('t'));
        let start = Instant::now();
        keyboard.poll(true, start);
        assert!(!keyboard.is_settling(start));

        keys.hold(ENTER);
        assert_eq!(keyboard.poll(true, start), [Key::Enter]);
        // The line is sent and the chat closed, with Enter still down.
        keyboard.poll(false, start);
        assert!(keyboard.is_settling(start));
        // A key pressed since is the game's, and does not prolong the wait.
        keys.hold(W);
        keyboard.poll(false, start);
        keys.release(ENTER);
        keyboard.poll(false, start);
        assert!(!keyboard.is_settling(start));

        // One never let go is not waited on for ever.
        keyboard.poll(true, start);
        keys.hold(ENTER);
        keyboard.poll(true, start);
        keyboard.poll(false, start);
        assert!(keyboard.is_settling(start));
        assert!(!keyboard.is_settling(start + SETTLE_LIMIT));
    }
}
