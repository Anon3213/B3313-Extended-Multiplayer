//! The minimap's turn in the session (`crate::minimap` draws it): reads the loaded area's walls
//! and warps when the game loads one, and draws the map again for the game to show while Mario
//! runs in a level and the player wants it, which `/map` and the map's key turn.

use crate::assist;
use crate::game::{self, Area};
use crate::interpolation::Interpolator;
use crate::mailbox::Mailbox;
use crate::minimap::{Mark, Terrain, View, draw};
use crate::people::{MapToggle, People};
use crate::ram::ProcessMemory;
use b3313_protocol::{PlayerId, Whereabouts};
use std::io;
use std::time::{Duration, Instant};

/// How often the map is drawn again: often enough that the marks move smoothly.
const DRAW_EVERY: Duration = Duration::from_millis(50);

/// The minimap from one turn of the session to the next: the loaded area's terrain and warps,
/// read again only when the game loads another area, and whether the game shows the map.
#[derive(Debug, Default)]
pub(crate) struct Minimap {
    /// Whether the player wants the map: as the session's options began it, then as `/map`
    /// and the map's key have turned it.
    wanted: bool,
    /// The area the terrain is of, by its place and the game's count of areas loaded.
    area: Option<((i16, u8), u32)>,
    terrain: Terrain,
    warps: Vec<Area>,
    drawn: Option<Instant>,
    shown: bool,
}

impl Minimap {
    pub(crate) fn new(wanted: bool) -> Self {
        Self {
            wanted,
            ..Self::default()
        }
    }

    /// Shows or hides the map as the player asked since the last turn, and draws it again if
    /// it is due, for the game to show while Mario runs in a level, or hides it otherwise.
    /// `drawn` holds the other players as the session draws them.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read or written.
    pub(crate) fn turn<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        people: &mut People,
        me: PlayerId,
        (drawn, playing): (&Interpolator, bool),
    ) -> io::Result<()> {
        for toggle in people.take_map_toggles() {
            self.wanted = !self.wanted;
            // The key is seen to work; the command, typed into the chat, is answered there.
            if toggle == MapToggle::Command {
                people.say_here(if self.wanted {
                    "The minimap is shown."
                } else {
                    "The minimap is hidden: /map shows it again."
                });
            }
            // Drawn at once when shown again, not up to a turn later.
            self.drawn = None;
        }
        if !self.wanted {
            return self.hide(mailbox);
        }
        let now = Instant::now();
        let is_due = self
            .drawn
            .is_none_or(|drawn| now.saturating_duration_since(drawn) >= DRAW_EVERY);
        if !is_due || !mailbox.is_ready()? {
            return Ok(());
        }
        self.drawn = Some(now);
        let ram = mailbox.ram();
        let current = game::place(ram)?;
        // As travel does, Mario is where the game published him only while he runs and the
        // place it published is the one it keeps: not while an area loads.
        let here = mailbox
            .read_local_player()?
            .map(|state| Whereabouts::of(&state))
            .filter(|here| playing && (here.level, here.area) == current);
        let Some(here) = here else {
            return self.hide(mailbox);
        };
        let area = (current, mailbox.area_loads()?);
        if self.area != Some(area) {
            let Some(surfaces) = game::static_surfaces(ram)? else {
                return self.hide(mailbox);
            };
            self.terrain = Terrain::of(&surfaces);
            self.warps = game::areas(ram)?;
            self.area = Some(area);
        }
        let mario = game::mario(ram)?;
        let view = View::new(here.position, mario.camera_yaw);
        let mut marks = Vec::new();
        for other in assist::named(people, me, &drawn.sample(now)) {
            let outfit = people.outfit_of(other.player);
            if other.place == current {
                marks.push(Mark::Player {
                    position: other.position,
                    yaw: other.yaw,
                    outfit,
                });
            } else if other.place.0 == current.0
                && let Some(position) = self.warp_toward(current.1, other.place.1)
            {
                marks.push(Mark::Through { position, outfit });
            }
        }
        marks.push(Mark::You { yaw: mario.yaw });
        mailbox.show_map(&draw(&self.terrain, &view, &marks))?;
        self.shown = true;
        Ok(())
    }

    /// Where a warp of area `from`, the loaded one, leads straight to area `to` of this level.
    fn warp_toward(&self, from: u8, to: u8) -> Option<[f32; 3]> {
        let level = self.area.map(|((level, _), _)| level)?;
        self.warps
            .iter()
            .find(|area| area.index == from)?
            .nodes
            .iter()
            .filter(|node| i16::from(node.to_level) == level && node.to_area == to)
            .find_map(|node| node.position)
    }

    fn hide<M: ProcessMemory>(&mut self, mailbox: &Mailbox<M>) -> io::Result<()> {
        if std::mem::take(&mut self.shown) {
            mailbox.hide_map()?;
        }
        Ok(())
    }
}
