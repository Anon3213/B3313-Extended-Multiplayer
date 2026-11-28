//! The minimap the game shows in the lower right corner: the walls and floors around Mario,
//! seen from above, and a mark for each player, for finding one another when near. The bridge
//! draws the whole picture and the game only shows it (`TrollEngine/Multiplayer/map.c`), as
//! the game has almost no room left for code.
//!
//! The map turns with the camera: up is the way the camera looks, which is the way the stick
//! pushed up walks Mario. Another player in the same area is an arrow where they are, or a dot
//! at the map's edge if they are beyond it; one in another area of the level is a dot at the
//! door, pipe or other warp of this area that leads there, if one does. Areas have their own
//! coordinates, so where in it they are cannot be drawn.

use crate::game::Surface;
use crate::layout::len;
use b3313_protocol::Outfit;
use std::f32::consts::TAU;

/// The picture's side, in pixels, and its colors.
const SIDE: usize = len::MAP_SIDE;
const COLORS: usize = len::MAP_COLORS;
const PIXEL_BYTES: usize = SIDE * SIDE / 2;
/// How much of the game's world a pixel covers: 3,200 units across the map, Mario being 160
/// tall.
const UNITS_PER_PIXEL: f32 = 50.0;
/// Walls are drawn that reach into this band about Mario's feet, and floors that reach into
/// the second: the floor he is on and those just below, not the storey above.
const WALLS_BELOW: f32 = 100.0;
const WALLS_ABOVE: f32 = 300.0;
const FLOORS_BELOW: f32 = 600.0;
const FLOORS_ABOVE: f32 = 120.0;
/// A surface facing up less than this either way is a wall, as the game itself sorts them.
const WALL_NORMAL: f32 = 0.01;
/// A mark at the map's edge stands this far in from it, so its outline shows whole.
const EDGE_INSET: f32 = 3.0;

/// The palette's places. Players' colors take what follows `FIRST_PLAYER`.
const BACKDROP: u8 = 0;
const FLOOR: u8 = 1;
const WALL: u8 = 2;
const FRAME: u8 = 3;
const OUTLINE: u8 = 4;
const YOU: u8 = 5;
const FIRST_PLAYER: u8 = 6;
const _: () = assert!(
    FIRST_PLAYER == 6,
    "COLORS_FIXED has a color for each place before it"
);
const COLORS_FIXED: [[u8; 3]; 6] = [
    [20, 22, 30],
    [70, 76, 96],
    [235, 235, 235],
    [150, 150, 160],
    [0, 0, 0],
    [255, 255, 255],
];

/// The game's 16-bit color: five bits each of red, green and blue, and a bit of alpha, here
/// always set: every pixel of the map is drawn.
fn color(rgb: [u8; 3]) -> u16 {
    let [red, green, blue] = rgb.map(|channel| u16::from(channel >> 3));
    red << 11 | green << 6 | blue << 1 | 1
}

/// An angle in the game's units, 0x10000 to a turn, in radians.
fn radians(angle: i16) -> f32 {
    f32::from(angle) * TAU / 65536.0
}

/// The picture's side and half of it as lengths on it, and its last pixel's column or row.
const SIDE_LENGTH: f32 = 64.0;
const _: () = assert!(SIDE == 64, "SIDE_LENGTH and LAST are SIDE's");
const HALF: f32 = SIDE_LENGTH / 2.0;
const LAST: i32 = 63;

/// The column or row of the pixel a place on the picture is in, which may be off it.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "a float's cast saturates, which puts a place far off the picture at a pixel off it \
              too, and every pixel is checked to be on it before it is drawn; there is no \
              checked conversion from a float"
)]
fn pixel_of(place: f32) -> i32 {
    place.floor() as i32
}

/// The left or top edge of the pixel in `pixel`'s column or row, as a place on the picture.
fn place_of(pixel: i32) -> f32 {
    i16::try_from(pixel).map_or(f32::NAN, f32::from)
}

/// One wall, as a line seen from above, and the heights it spans.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Wall {
    ends: [[f32; 2]; 2],
    low: f32,
    high: f32,
}

/// One floor triangle, seen from above, and the heights it spans.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Floor {
    corners: [[f32; 2]; 3],
    low: f32,
    high: f32,
}

/// The walls and floors of the loaded area, from its surfaces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Terrain {
    walls: Vec<Wall>,
    floors: Vec<Floor>,
}

impl Terrain {
    /// Sorts the area's surfaces into walls and floors; ceilings are left out. A wall is drawn
    /// as the longest of its triangle's sides seen from above, which for an upright wall is
    /// the whole of it.
    #[must_use]
    pub fn of(surfaces: &[Surface]) -> Self {
        let mut terrain = Self::default();
        for surface in surfaces {
            let (low, high) = span(surface.corners.map(|[_, y, _]| y));
            let flat = surface.corners.map(|[x, _, z]| [x, z]);
            if surface.normal_y.abs() <= WALL_NORMAL {
                let [first, second, third] = flat;
                let longest = [[first, second], [second, third], [third, first]]
                    .into_iter()
                    .max_by(|one, other| length(*one).total_cmp(&length(*other)));
                if let Some(ends) = longest {
                    terrain.walls.push(Wall { ends, low, high });
                }
            } else if surface.normal_y > WALL_NORMAL {
                terrain.floors.push(Floor {
                    corners: flat,
                    low,
                    high,
                });
            }
        }
        terrain
    }
}

fn length([[ax, az], [bx, bz]]: [[f32; 2]; 2]) -> f32 {
    (bx - ax).hypot(bz - az)
}

/// The least and the greatest of three values.
fn span(values: [f32; 3]) -> (f32, f32) {
    let low = values.iter().copied().fold(f32::INFINITY, f32::min);
    let high = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    (low, high)
}

/// Where the map is centered and which way it is turned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Mario's position, the map's center.
    pub center: [f32; 3],
    /// Up on the map, in the game's angle units: 0 is toward +Z, 0x4000 toward +X.
    pub up: i16,
}

impl View {
    /// The view for Mario at `position`, turned so that up is the way the camera looks; with
    /// no camera to tell of, up is +Z. The camera's yaw is from what it looks at to itself, so
    /// it looks the opposite way.
    #[must_use]
    pub fn new(position: [f32; 3], camera_yaw: Option<i16>) -> Self {
        Self {
            center: position,
            up: camera_yaw.map_or(0, |yaw| yaw.wrapping_add(i16::MIN)),
        }
    }

    /// Up and right on the map as directions in the world, each `[x, z]`. In the game, +X is
    /// to the left of one looking toward +Z.
    fn axes(&self) -> ([f32; 2], [f32; 2]) {
        let (sin, cos) = radians(self.up).sin_cos();
        ([sin, cos], [-cos, sin])
    }

    /// The world's point `[x, z]` on the picture, as `[column, row]` from its top left corner,
    /// in pixels: Mario is at the middle.
    fn place(&self, [x, z]: [f32; 2]) -> [f32; 2] {
        let (up, right) = self.axes();
        let (dx, dz) = (x - self.center[0], z - self.center[2]);
        [
            HALF + (dx * right[0] + dz * right[1]) / UNITS_PER_PIXEL,
            HALF - (dx * up[0] + dz * up[1]) / UNITS_PER_PIXEL,
        ]
    }

    /// The way `yaw` faces on the picture, as a step of one pixel `[column, row]`.
    fn facing(&self, yaw: i16) -> [f32; 2] {
        let (up, right) = self.axes();
        let (sin, cos) = radians(yaw).sin_cos();
        [
            sin * right[0] + cos * right[1],
            -(sin * up[0] + cos * up[1]),
        ]
    }

    /// Whether the band of heights from `low` to `high` reaches the band about Mario's feet
    /// from `below` under them to `above` over them.
    fn reaches(&self, low: f32, high: f32, below: f32, above: f32) -> bool {
        high >= self.center[1] - below && low <= self.center[1] + above
    }
}

/// What a mark on the map stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    /// The local Mario, at the middle, facing `yaw`.
    You { yaw: i16 },
    /// Another player in this area: an arrow where they are, or a dot at the edge if that is
    /// beyond it.
    Player {
        position: [f32; 3],
        yaw: i16,
        outfit: Outfit,
    },
    /// Another player in another area of the level, through the warp at `position`: a dot.
    Through { position: [f32; 3], outfit: Outfit },
}

/// The picture the game shows, in its own formats: two pixels a byte, the left one high, row
/// by row from the top, and the palette's colors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub pixels: [u8; PIXEL_BYTES],
    pub palette: [u16; COLORS],
}

impl Picture {
    /// The pixels and then the palette, as they lie in the game's memory (`struct MpMap`).
    #[must_use]
    pub fn bytes(&self) -> Vec<u8> {
        let mut bytes = self.pixels.to_vec();
        for color in self.palette {
            bytes.extend(color.to_be_bytes());
        }
        bytes
    }
}

/// The picture being drawn: a palette place for each pixel.
struct Canvas([u8; SIDE * SIDE]);

impl Canvas {
    fn set(&mut self, column: i32, row: i32, ink: u8) {
        let (Ok(column), Ok(row)) = (usize::try_from(column), usize::try_from(row)) else {
            return;
        };
        if column < SIDE
            && let Some(pixel) = self.0.get_mut(row * SIDE + column)
        {
            *pixel = ink;
        }
    }

    fn get(&self, column: i32, row: i32) -> Option<u8> {
        let column = usize::try_from(column)
            .ok()
            .filter(|&column| column < SIDE)?;
        let row = usize::try_from(row).ok()?;
        self.0.get(row * SIDE + column).copied()
    }

    /// Fills the pixels whose centers are inside the triangle `corners`, in either winding,
    /// and returns which, as `[column, row]`.
    fn triangle(&mut self, corners: [[f32; 2]; 3], ink: u8) -> Vec<[i32; 2]> {
        let [a, b, c] = corners;
        let edge = |p: [f32; 2], q: [f32; 2], r: [f32; 2]| {
            (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
        };
        let area = edge(a, b, c);
        if area == 0.0 || !area.is_finite() {
            return Vec::new();
        }
        // The pixels the triangle's box covers, on the picture.
        let pixels = |values: [f32; 3]| {
            let (low, high) = span(values);
            (
                pixel_of(low.clamp(0.0, SIDE_LENGTH)),
                pixel_of(high.clamp(0.0, SIDE_LENGTH).ceil()),
            )
        };
        let (left, right) = pixels(corners.map(|[column, _]| column));
        let (top, bottom) = pixels(corners.map(|[_, row]| row));
        let mut filled = Vec::new();
        for row in top..bottom {
            for column in left..right {
                let center = [place_of(column) + 0.5, place_of(row) + 0.5];
                let inside = [edge(a, b, center), edge(b, c, center), edge(c, a, center)]
                    .iter()
                    .all(|&side| side * area >= 0.0);
                if inside {
                    self.set(column, row, ink);
                    filled.push([column, row]);
                }
            }
        }
        filled
    }

    /// Draws the line from `from` to `to`, the part of it on the picture.
    fn line(&mut self, from: [f32; 2], to: [f32; 2], ink: u8) {
        let Some((from, to)) = clip(from, to) else {
            return;
        };
        let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
        // A clipped line is at most the picture's diagonal long, so the steps are few.
        let steps = dx.abs().max(dy.abs()).ceil().max(1.0);
        for step in 0..=pixel_of(steps) {
            let along = place_of(step) / steps;
            self.set(
                pixel_of(from[0] + dx * along),
                pixel_of(from[1] + dy * along),
                ink,
            );
        }
    }

    /// Outlines `filled`: every pixel beside one of them, not itself among them.
    fn outline(&mut self, filled: &[[i32; 2]]) {
        for &[column, row] in filled {
            for (dc, dr) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (c, r) = (column + dc, row + dr);
                if !filled.contains(&[c, r]) && self.get(c, r).is_some() {
                    self.set(c, r, OUTLINE);
                }
            }
        }
    }

    /// An arrow at `at` pointing along `facing`, a step of one pixel.
    fn arrow(&mut self, at: [f32; 2], facing: [f32; 2], ink: u8) {
        let [fx, fy] = facing;
        let [px, py] = [-fy, fx];
        let point = |ahead: f32, aside: f32| {
            [
                at[0] + fx * ahead + px * aside,
                at[1] + fy * ahead + py * aside,
            ]
        };
        let mut filled = self.triangle([point(4.0, 0.0), point(-2.5, 3.0), point(-2.5, -3.0)], ink);
        // However it is turned, the pixel it stands on is part of it.
        let middle = [pixel_of(at[0]), pixel_of(at[1])];
        if !filled.contains(&middle) {
            self.set(middle[0], middle[1], ink);
            filled.push(middle);
        }
        self.outline(&filled);
    }

    /// A dot at `at`, three pixels across.
    fn dot(&mut self, at: [f32; 2], ink: u8) {
        let [column, row] = [pixel_of(at[0]), pixel_of(at[1])];
        let mut filled = Vec::new();
        for (dc, dr) in [
            (-1, 0),
            (0, 0),
            (1, 0),
            (0, -1),
            (0, 1),
            (-1, -1),
            (1, -1),
            (-1, 1),
            (1, 1),
        ] {
            self.set(column + dc, row + dr, ink);
            filled.push([column + dc, row + dr]);
        }
        self.outline(&filled);
    }

    fn frame(&mut self) {
        for i in 0..=LAST {
            for (column, row) in [(i, 0), (i, LAST), (0, i), (LAST, i)] {
                self.set(column, row, FRAME);
            }
        }
    }

    fn packed(&self) -> [u8; PIXEL_BYTES] {
        let mut pixels = [0; PIXEL_BYTES];
        for (byte, pair) in pixels.iter_mut().zip(self.0.as_chunks::<2>().0) {
            *byte = pair[0] << 4 | (pair[1] & 0x0F);
        }
        pixels
    }
}

/// The part of the line from `from` to `to` inside the picture, or None if none is
/// (Liang and Barsky's clipping).
fn clip(from: [f32; 2], to: [f32; 2]) -> Option<([f32; 2], [f32; 2])> {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let side = SIDE_LENGTH;
    let (mut enter, mut leave) = (0.0_f32, 1.0_f32);
    for (p, q) in [
        (-dx, from[0]),
        (dx, side - from[0]),
        (-dy, from[1]),
        (dy, side - from[1]),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                enter = enter.max(t);
            } else {
                leave = leave.min(t);
            }
        }
    }
    (enter <= leave && enter.is_finite() && leave.is_finite()).then(|| {
        (
            [from[0] + dx * enter, from[1] + dy * enter],
            [from[0] + dx * leave, from[1] + dy * leave],
        )
    })
}

/// Where a mark at `place` on the picture goes: there if it is on it, else at the edge on the
/// way to it from the middle. Returns whether it was moved to the edge.
fn on_picture(place: [f32; 2]) -> ([f32; 2], bool) {
    let (dx, dy) = (place[0] - HALF, place[1] - HALF);
    let reach = HALF - EDGE_INSET;
    let farthest = dx.abs().max(dy.abs());
    if farthest <= reach || !farthest.is_finite() {
        (place, false)
    } else {
        let scale = reach / farthest;
        ([HALF + dx * scale, HALF + dy * scale], true)
    }
}

/// Draws the map for `view` from `terrain` with `marks`, the local Mario's last, on top.
#[must_use]
pub fn draw(terrain: &Terrain, view: &View, marks: &[Mark]) -> Picture {
    let mut canvas = Canvas([BACKDROP; SIDE * SIDE]);
    // Seen turned, the picture's corners reach out to its half diagonal; this reaches past.
    let reach = SIDE_LENGTH * UNITS_PER_PIXEL;
    // Whether the box about these points reaches the map: a floor or wall far larger than it
    // can cover it with every corner beyond it.
    let near = |points: &[[f32; 2]]| {
        let reaches = |values: &mut dyn Iterator<Item = f32>, middle: f32| {
            let (low, high) = values
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), value| {
                    (low.min(value), high.max(value))
                });
            high > middle - reach && low < middle + reach
        };
        reaches(&mut points.iter().map(|[x, _]| *x), view.center[0])
            && reaches(&mut points.iter().map(|[_, z]| *z), view.center[2])
    };
    for floor in &terrain.floors {
        if view.reaches(floor.low, floor.high, FLOORS_BELOW, FLOORS_ABOVE) && near(&floor.corners) {
            canvas.triangle(floor.corners.map(|corner| view.place(corner)), FLOOR);
        }
    }
    for wall in &terrain.walls {
        if view.reaches(wall.low, wall.high, WALLS_BELOW, WALLS_ABOVE) && near(&wall.ends) {
            canvas.line(view.place(wall.ends[0]), view.place(wall.ends[1]), WALL);
        }
    }
    canvas.frame();

    let mut palette = [0; COLORS];
    for (place, rgb) in palette.iter_mut().zip(COLORS_FIXED) {
        *place = color(rgb);
    }
    // Each outfit on the map takes the next palette place; past the last, they share it.
    let mut outfits: Vec<Outfit> = Vec::new();
    let mut ink_of = |outfit: Outfit, palette: &mut [u16; COLORS]| {
        let index = outfits
            .iter()
            .position(|&known| known == outfit)
            .unwrap_or_else(|| {
                outfits.push(outfit);
                outfits.len() - 1
            });
        let ink = (usize::from(FIRST_PLAYER) + index).min(COLORS - 1);
        if let Some(place) = palette.get_mut(ink) {
            *place = color(outfit.cap());
        }
        u8::try_from(ink).unwrap_or(FIRST_PLAYER)
    };
    let flat = |[x, _, z]: [f32; 3]| [x, z];
    for mark in marks {
        match *mark {
            Mark::You { .. } => {}
            Mark::Player {
                position,
                yaw,
                outfit,
            } => {
                let ink = ink_of(outfit, &mut palette);
                match on_picture(view.place(flat(position))) {
                    (at, false) => canvas.arrow(at, view.facing(yaw), ink),
                    (at, true) => canvas.dot(at, ink),
                }
            }
            Mark::Through { position, outfit } => {
                let ink = ink_of(outfit, &mut palette);
                canvas.dot(on_picture(view.place(flat(position))).0, ink);
            }
        }
    }
    for mark in marks {
        if let Mark::You { yaw } = *mark {
            canvas.arrow(view.place(flat(view.center)), view.facing(yaw), YOU);
        }
    }
    Picture {
        pixels: canvas.packed(),
        palette,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BACKDROP, FIRST_PLAYER, FLOOR, FRAME, HALF, Mark, OUTLINE, Picture, SIDE, Terrain,
        UNITS_PER_PIXEL, View, WALL, YOU, color, draw,
    };
    use crate::game::Surface;
    use b3313_protocol::Outfit;

    /// The palette place of the pixel at `[column, row]`.
    fn pixel(picture: &Picture, column: usize, row: usize) -> Option<u8> {
        let byte = picture.pixels.get(row * SIDE / 2 + column / 2)?;
        Some(if column.is_multiple_of(2) {
            byte >> 4
        } else {
            byte & 0x0F
        })
    }

    fn palette(picture: &Picture, ink: u8) -> Option<u16> {
        picture.palette.get(usize::from(ink)).copied()
    }

    fn wall(from: [f32; 2], to: [f32; 2], low: f32, high: f32) -> Surface {
        Surface {
            corners: [
                [from[0], low, from[1]],
                [to[0], low, to[1]],
                [to[0], high, to[1]],
            ],
            normal_y: 0.0,
        }
    }

    fn floor(corners: [[f32; 2]; 3], y: f32) -> Surface {
        Surface {
            corners: corners.map(|[x, z]| [x, y, z]),
            normal_y: 1.0,
        }
    }

    /// Up on the map is +Z: the camera stands on the -Z side, looking toward +Z.
    fn looking_north(center: [f32; 3]) -> View {
        View::new(center, Some(i16::MIN))
    }

    #[test]
    fn the_camera_looking_toward_plus_z_puts_plus_z_up_and_plus_x_left() {
        let view = looking_north([1000.0, 0.0, 1000.0]);
        let half = HALF;
        let near = |[column, row]: [f32; 2], [c, r]: [f32; 2]| {
            (column - c).abs() < 1e-3 && (row - r).abs() < 1e-3
        };
        assert!(near(
            view.place([1000.0, 1000.0 + 10.0 * UNITS_PER_PIXEL]),
            [half, half - 10.0]
        ));
        assert!(near(
            view.place([1000.0 + 10.0 * UNITS_PER_PIXEL, 1000.0]),
            [half - 10.0, half]
        ));
        // Facing the way the camera looks is up the picture.
        let [column, row] = view.facing(0);
        assert!(column.abs() < 1e-6 && (row + 1.0).abs() < 1e-6);
        // A camera turned a quarter of the way about turns the map with it.
        let east = View::new([0.0; 3], Some(i16::MIN + 0x4000));
        assert!(near(
            east.place([10.0 * UNITS_PER_PIXEL, 0.0]),
            [half, half - 10.0]
        ));
        // With no camera, up is +Z as well.
        assert_eq!(View::new([0.0; 3], None).up, 0);
    }

    #[test]
    fn walls_and_floors_are_sorted_from_the_surfaces_and_ceilings_left_out() {
        let ceiling = Surface {
            corners: [[0.0, 300.0, 0.0], [100.0, 300.0, 0.0], [0.0, 300.0, 100.0]],
            normal_y: -1.0,
        };
        let terrain = Terrain::of(&[
            wall([0.0, 0.0], [500.0, 0.0], 0.0, 400.0),
            floor([[0.0, 0.0], [100.0, 0.0], [0.0, 100.0]], 0.0),
            ceiling,
        ]);
        assert_eq!((terrain.walls.len(), terrain.floors.len()), (1, 1));
        // An upright wall's longest side seen from above is the whole wall.
        let ends = terrain.walls.first().map(|wall| wall.ends);
        assert_eq!(ends.map(super::length), Some(500.0));
    }

    #[test]
    fn a_wall_at_marios_height_is_drawn_and_one_a_storey_above_is_not() {
        let center = [0.0, 0.0, 0.0];
        let ahead = 10.0 * UNITS_PER_PIXEL;
        let terrain = Terrain::of(&[
            wall([-1000.0, ahead], [1000.0, ahead], 0.0, 400.0),
            wall([-1000.0, -ahead], [1000.0, -ahead], 1000.0, 1400.0),
        ]);
        let picture = draw(&terrain, &looking_north(center), &[]);
        let half = SIDE / 2;
        assert_eq!(pixel(&picture, half + 5, half - 10), Some(WALL));
        assert_eq!(pixel(&picture, half + 5, half + 10), Some(BACKDROP));
        assert_eq!(pixel(&picture, 0, 0), Some(FRAME));
    }

    #[test]
    fn the_floor_under_mario_is_filled_and_one_overhead_is_not() {
        let big = 5000.0;
        let terrain = Terrain::of(&[
            floor([[-big, -big], [big, -big], [-big, big]], 0.0),
            floor([[big, big], [big, -big], [-big, big]], 500.0),
        ]);
        let picture = draw(&terrain, &looking_north([0.0, 0.0, 0.0]), &[]);
        // With +X to the left, the first floor's half is the lower right and upper left of the
        // picture's diagonal; the second, overhead, is left as the backdrop.
        assert_eq!(pixel(&picture, 40, 40), Some(FLOOR));
        assert_eq!(pixel(&picture, 20, 20), Some(BACKDROP));
    }

    #[test]
    fn players_are_arrows_in_their_colors_and_those_beyond_are_dots_at_the_edge()
    -> Result<(), &'static str> {
        let view = looking_north([0.0, 0.0, 0.0]);
        let (Some(red), Some(green)) = (Outfit::named("red"), Outfit::named("green")) else {
            return Err("red and green are outfits");
        };
        let near = 15.0 * UNITS_PER_PIXEL;
        let picture = draw(
            &Terrain::default(),
            &view,
            &[
                Mark::Player {
                    position: [0.0, 0.0, near],
                    yaw: 0,
                    outfit: red,
                },
                Mark::Through {
                    position: [0.0, 0.0, -100_000.0],
                    outfit: green,
                },
                Mark::You { yaw: 0 },
            ],
        );
        let half = SIDE / 2;
        assert_eq!(pixel(&picture, half, half), Some(YOU));
        assert_eq!(pixel(&picture, half, half - 15), Some(FIRST_PLAYER));
        assert_eq!(palette(&picture, FIRST_PLAYER), Some(color(red.cap())));
        // Far behind: a dot at the bottom edge, in the next color, outlined.
        let bottom = SIDE - 4;
        assert_eq!(pixel(&picture, half, bottom), Some(FIRST_PLAYER + 1));
        assert_eq!(
            palette(&picture, FIRST_PLAYER + 1),
            Some(color(green.cap()))
        );
        assert_eq!(pixel(&picture, half, bottom + 3), Some(OUTLINE));
        Ok(())
    }

    #[test]
    fn the_picture_packs_two_pixels_a_byte_left_high_then_the_palette() {
        let picture = draw(&Terrain::default(), &looking_north([0.0; 3]), &[]);
        let bytes = picture.bytes();
        assert_eq!(bytes.len(), SIDE * SIDE / 2 + 32);
        // The frame's top row, then a row of backdrop inside it.
        assert_eq!(bytes.first(), Some(&(FRAME << 4 | FRAME)));
        assert_eq!(bytes.get(SIDE / 2 + 1), Some(&(BACKDROP << 4 | BACKDROP)));
        assert_eq!(color([255, 255, 255]), 0xFFFF);
        assert_eq!(color([0, 0, 0]), 0x0001);
    }
}
