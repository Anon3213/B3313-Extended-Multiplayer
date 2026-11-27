//! The game's own state, outside the mailbox: Mario, the camera, the level and its areas with
//! their warps, and the variables a warp is started with. For bots (`bots`) and the bridge's
//! travel (`travel`), which see and move the local Mario.
//!
//! Every place here is the game's own, kept where vanilla Super Mario 64 keeps it, and the same
//! in every version of B3313 this program plays: the test scripts read and write them in all
//! three (`scripts/game_state.py`), and `TrollEngine/trollfinity.asm` names them. What the game
//! keeps behind a pointer is read only once the pointer is checked to be into RAM.

use crate::ram::{N64Ram, ProcessMemory};
use std::io;

/// `gMarioStates[0]`, the local Mario, and the places in it that are read here.
const MARIO_STATE: u32 = 0x8033_B170;
const MARIO_STATE_SIZE: usize = 0xB0;
const MARIO_ACTION: usize = 0x0C;
const MARIO_FACE_YAW: usize = 0x2E;
const MARIO_POS: usize = 0x3C;
const MARIO_POS_ADDRESS: u32 = MARIO_STATE + 0x3C;
const MARIO_VEL: usize = 0x48;
const MARIO_FORWARD_VEL: usize = 0x54;
const MARIO_AREA: usize = 0x90;
const MARIO_COINS: usize = 0xA8;
const MARIO_STARS: usize = 0xAA;
const MARIO_LIVES: usize = 0xAD;
const MARIO_HEALTH: usize = 0xAE;
/// `struct Camera`: its `yaw` is the half-word after two bytes of modes. It is the angle from
/// what the camera looks at to the camera, which the game turns the stick by.
const AREA_CAMERA: u32 = 0x24;
const CAMERA_YAW_WORD: u32 = 0x00;
/// `gCurrLevelNum`, an s16 in the upper half of its word, and `gCurrAreaIndex`, an s16 in the
/// lower half of its.
const LEVEL_WORD: u32 = 0x8032_DDF8;
const AREA_WORD: u32 = 0x8033_BAC8;
/// `gAreas`, which B3313 points at a table of its own with room for sixteen areas, from area
/// 0; and `gCurrentArea`, the one loaded.
const AREAS_POINTER: u32 = 0x8032_DDC8;
const CURRENT_AREA_POINTER: u32 = 0x8032_DDCC;
pub const AREA_SLOTS: u8 = 16;
const AREA_SIZE: u32 = 0x3C;
/// In `struct Area`: the scene, which only an area the level has has, and the warp nodes.
const AREA_SCENE: u32 = 0x04;
const AREA_WARP_NODES: u32 = 0x14;
/// `struct ObjectWarpNode`: the node, `{id, destLevel, destArea, destNode}`, its object, and
/// the next node.
const WARP_NODE_SIZE: usize = 12;
/// `WARP_CHECKPOINT`, which a level script adds to a node's level.
const WARP_CHECKPOINT: u8 = 0x80;
/// No area has more nodes than this; a list longer is not one the game made.
const WARP_NODES_MOST: usize = 64;
/// An object's `oPosX`, `oPosY` and `oPosZ`, and its `activeFlags`, of which bit 0 is in use.
const OBJECT_POS: u32 = 0xA0;
const OBJECT_ACTIVE_FLAGS_WORD: u32 = 0x74;
/// The object pool, where every object the game runs lives.
const OBJECT_POOL: u32 = 0x8033_D488;
const OBJECT_SIZE: u32 = 0x260;
const OBJECT_POOL_SLOTS: u32 = 240;
/// `sWarpDest`: `{u8 type, levelNum, areaIdx, nodeId; u32 arg}`.
const WARP_DEST: u32 = 0x8033_B248;
const WARP_TYPE_NOT_WARPING: u8 = 0;
const WARP_TYPE_CHANGE_LEVEL: u8 = 1;
/// The level the game starts afresh when no warp is under way (`D_80339EE0` in the engine's
/// names), an s16 in the upper half of its word; `sDelayedWarpOp`, the lower half, is left as
/// it is.
const LEVEL_TO_START_WORD: u32 = 0x8033_B250;
/// `sCurrPlayMode`, an s16 in the upper half of its word, and the mode that acts on a warp.
const PLAY_MODE_WORD: u32 = 0x8033_B238;
const PLAY_MODE_CHANGE_LEVEL: u16 = 4;
/// The node a painting enters a course by, which most of B3313's areas have.
pub const PAINTING_ENTRY_NODE: u8 = 0x0A;
/// Mario's action flags: in the air, and in water.
const ACTION_AIR: u32 = 0x0000_0800;
const ACTION_SWIMMING: u32 = 0x0000_2000;

/// The local Mario as the game has him now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mario {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub forward_speed: f32,
    /// The way he faces, in the game's angle units: 0 is toward +Z, 0x4000 toward +X.
    pub yaw: i16,
    pub action: u32,
    /// In 256ths of a wedge of his power meter: 0x880 is full.
    pub health: i16,
    pub coins: i16,
    pub stars: i16,
    pub lives: i8,
    /// The camera's angle, as the game turns the stick by: from what it looks at to itself.
    /// None while the game has no camera to tell of.
    pub camera_yaw: Option<i16>,
}

impl Mario {
    #[must_use]
    pub const fn is_in_air(&self) -> bool {
        self.action & ACTION_AIR != 0
    }

    #[must_use]
    pub const fn is_swimming(&self) -> bool {
        self.action & ACTION_SWIMMING != 0
    }
}

/// One way out of an area, as the level has it: a warp node, and where it leads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarpNode {
    pub id: u8,
    pub to_level: u8,
    pub to_area: u8,
    pub to_node: u8,
    /// Where its object is, for a node of the area loaded now: where Mario arrives by it, and,
    /// for a door or a pipe, what takes him on. None for the other areas, whose objects are
    /// not loaded, and for a node with no object.
    pub position: Option<[f32; 3]>,
}

/// One area of the level loaded now, and its warp nodes.
#[derive(Clone, Debug, PartialEq)]
pub struct Area {
    pub index: u8,
    pub nodes: Vec<WarpNode>,
}

/// `value` as an address into RAM that a word may be read at, or None.
const fn ram_pointer(value: u32) -> Option<u32> {
    if value >= 0x8000_0000 && value < 0x8000_0000 + crate::ram::RAM_SIZE && value.is_multiple_of(4)
    {
        Some(value)
    } else {
        None
    }
}

fn float(bytes: &[u8], at: usize) -> f32 {
    bytes
        .get(at..at + 4)
        .and_then(|word| word.try_into().ok())
        .map_or(f32::NAN, f32::from_be_bytes)
}

fn half(bytes: &[u8], at: usize) -> i16 {
    bytes
        .get(at..at + 2)
        .and_then(|word| word.try_into().ok())
        .map_or(0, i16::from_be_bytes)
}

fn word(bytes: &[u8], at: usize) -> u32 {
    bytes
        .get(at..at + 4)
        .and_then(|word| word.try_into().ok())
        .map_or(0, u32::from_be_bytes)
}

/// The level and area the game is in. The level is not one until the game has loaded one.
///
/// # Errors
/// Fails if the game's memory cannot be read.
pub fn place<M: ProcessMemory>(ram: &N64Ram<'_, M>) -> io::Result<(i16, u8)> {
    let [level_high, level_low, ..] = ram.read_u32(LEVEL_WORD)?.to_be_bytes();
    let [.., area_high, area_low] = ram.read_u32(AREA_WORD)?.to_be_bytes();
    let area = i16::from_be_bytes([area_high, area_low]);
    Ok((
        i16::from_be_bytes([level_high, level_low]),
        u8::try_from(area).unwrap_or(u8::MAX),
    ))
}

/// The local Mario.
///
/// # Errors
/// Fails if the game's memory cannot be read.
pub fn mario<M: ProcessMemory>(ram: &N64Ram<'_, M>) -> io::Result<Mario> {
    let mut state = [0; MARIO_STATE_SIZE];
    ram.read(MARIO_STATE, &mut state)?;
    let vector = |at: usize| {
        [
            float(&state, at),
            float(&state, at + 4),
            float(&state, at + 8),
        ]
    };
    let camera_yaw = match ram_pointer(word(&state, MARIO_AREA)) {
        Some(area) => camera_yaw(ram, area)?,
        None => None,
    };
    Ok(Mario {
        position: vector(MARIO_POS),
        velocity: vector(MARIO_VEL),
        forward_speed: float(&state, MARIO_FORWARD_VEL),
        yaw: half(&state, MARIO_FACE_YAW),
        action: word(&state, MARIO_ACTION),
        health: half(&state, MARIO_HEALTH),
        coins: half(&state, MARIO_COINS),
        stars: half(&state, MARIO_STARS),
        lives: state
            .get(MARIO_LIVES)
            .map_or(0, |&lives| i8::from_be_bytes([lives])),
        camera_yaw,
    })
}

fn camera_yaw<M: ProcessMemory>(ram: &N64Ram<'_, M>, area: u32) -> io::Result<Option<i16>> {
    let Some(camera) = ram_pointer(ram.read_u32(area + AREA_CAMERA)?) else {
        return Ok(None);
    };
    let [.., high, low] = ram.read_u32(camera + CAMERA_YAW_WORD)?.to_be_bytes();
    Ok(Some(i16::from_be_bytes([high, low])))
}

/// Whether `object` is an object of the pool that is in use.
fn is_live_object<M: ProcessMemory>(ram: &N64Ram<'_, M>, object: u32) -> io::Result<bool> {
    let pool_end = OBJECT_POOL + OBJECT_POOL_SLOTS * OBJECT_SIZE;
    if !(OBJECT_POOL..pool_end).contains(&object)
        || !(object - OBJECT_POOL).is_multiple_of(OBJECT_SIZE)
    {
        return Ok(false);
    }
    // `activeFlags` is the s16 in the upper half of its word.
    Ok(ram.read_u32(object + OBJECT_ACTIVE_FLAGS_WORD)? & 0x0001_0000 != 0)
}

/// Every area of the level loaded now, with its warp nodes; where the nodes of the area loaded
/// now have their objects.
///
/// # Errors
/// Fails if the game's memory cannot be read.
pub fn areas<M: ProcessMemory>(ram: &N64Ram<'_, M>) -> io::Result<Vec<Area>> {
    let Some(table) = ram_pointer(ram.read_u32(AREAS_POINTER)?) else {
        return Ok(Vec::new());
    };
    let current = ram.read_u32(CURRENT_AREA_POINTER)?;
    let mut areas = Vec::new();
    for index in 0..AREA_SLOTS {
        let area = table + u32::from(index) * AREA_SIZE;
        if ram.read_u32(area + AREA_SCENE)? == 0 {
            continue;
        }
        let nodes = warp_nodes(ram, ram.read_u32(area + AREA_WARP_NODES)?, area == current)?;
        areas.push(Area { index, nodes });
    }
    Ok(areas)
}

fn warp_nodes<M: ProcessMemory>(
    ram: &N64Ram<'_, M>,
    first: u32,
    is_loaded: bool,
) -> io::Result<Vec<WarpNode>> {
    let mut nodes = Vec::new();
    let mut next = ram_pointer(first);
    while let Some(at) = next
        && nodes.len() < WARP_NODES_MOST
    {
        let mut record = [0; WARP_NODE_SIZE];
        ram.read(at, &mut record)?;
        let [id, to_level, to_area, to_node, ..] = record;
        // A node's level carries the flag of a checkpoint in its top bit.
        let to_level = to_level & !WARP_CHECKPOINT;
        let object = word(&record, 4);
        let position = if is_loaded && is_live_object(ram, object)? {
            let mut pos = [0; 12];
            ram.read(object + OBJECT_POS, &mut pos)?;
            Some([float(&pos, 0), float(&pos, 4), float(&pos, 8)])
        } else {
            None
        };
        nodes.push(WarpNode {
            id,
            to_level,
            to_area,
            to_node,
            position,
        });
        next = ram_pointer(word(&record, 8));
    }
    Ok(nodes)
}

/// Writes the upper half of the word at `address`, leaving the lower half as it is.
fn write_upper_half<M: ProcessMemory>(
    ram: &N64Ram<'_, M>,
    address: u32,
    value: u16,
) -> io::Result<()> {
    let lower = ram.read_u32(address)? & 0xFFFF;
    ram.write(address, &(u32::from(value) << 16 | lower).to_be_bytes())
}

/// Has the game start `level` afresh, as it starts one from the file select: Mario where the
/// level's script puts him, in the area it names. No warp node is needed, which a level the
/// game has not loaded cannot be asked about.
///
/// # Errors
/// Fails if the game's memory cannot be written.
pub fn start_level<M: ProcessMemory>(ram: &N64Ram<'_, M>, level: i16) -> io::Result<()> {
    ram.write(WARP_DEST, &[WARP_TYPE_NOT_WARPING, 0, 0, 0, 0, 0, 0, 0])?;
    write_upper_half(ram, LEVEL_TO_START_WORD, level.cast_unsigned())?;
    write_upper_half(ram, PLAY_MODE_WORD, PLAY_MODE_CHANGE_LEVEL)
}

/// Has the game take Mario to warp node `node` of `area` of `level`, which must be a node that
/// area has: one it lacks stops the game.
///
/// # Errors
/// Fails if the game's memory cannot be written.
pub fn warp<M: ProcessMemory>(
    ram: &N64Ram<'_, M>,
    level: u8,
    area: u8,
    node: u8,
) -> io::Result<()> {
    ram.write(
        WARP_DEST,
        &[WARP_TYPE_CHANGE_LEVEL, level, area, node, 0, 0, 0, 0],
    )?;
    write_upper_half(ram, PLAY_MODE_WORD, PLAY_MODE_CHANGE_LEVEL)
}

/// Puts the local Mario at `position`, from where he falls to the floor below it.
///
/// # Errors
/// Fails if the game's memory cannot be written.
pub fn teleport<M: ProcessMemory>(ram: &N64Ram<'_, M>, position: [f32; 3]) -> io::Result<()> {
    let mut bytes = [0; 12];
    for (slot, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(position) {
        slot.copy_from_slice(&value.to_be_bytes());
    }
    ram.write(MARIO_POS_ADDRESS, &bytes)
}

#[cfg(test)]
mod tests {
    use super::{
        AREA_SCENE, AREA_SIZE, AREA_WARP_NODES, AREA_WORD, AREAS_POINTER, CURRENT_AREA_POINTER,
        LEVEL_TO_START_WORD, LEVEL_WORD, MARIO_STATE, OBJECT_ACTIVE_FLAGS_WORD, OBJECT_POOL,
        OBJECT_POS, OBJECT_SIZE, PLAY_MODE_WORD, WARP_DEST, areas, mario, place, start_level,
        teleport, warp,
    };
    use crate::ram::N64Ram;
    use crate::ram::testing::FakeProcess;
    use std::io;

    const BASE: usize = 0x1000;
    /// All of RAM, as `crate::ram::RAM_SIZE` is.
    const RAM_BYTES: usize = 0x0080_0000;

    fn write(ram: &N64Ram<'_, FakeProcess>, address: u32, value: u32) -> io::Result<()> {
        ram.write(address, &value.to_be_bytes())
    }

    #[test]
    fn the_place_is_read_from_the_halves_of_two_words() -> io::Result<()> {
        let process = FakeProcess::new(BASE, RAM_BYTES);
        let ram = N64Ram::new(&process, BASE);
        write(&ram, LEVEL_WORD, 0x0009_ABCD)?;
        write(&ram, AREA_WORD, 0x1234_0003)?;
        assert_eq!(place(&ram)?, (9, 3));
        Ok(())
    }

    #[test]
    fn mario_is_read_with_the_camera_his_area_has() -> io::Result<()> {
        let process = FakeProcess::new(BASE, RAM_BYTES);
        let ram = N64Ram::new(&process, BASE);
        let area = 0x8020_0000;
        let camera = 0x8020_1000;
        ram.write(
            MARIO_STATE + 0x3C,
            &[0x44, 0x7A, 0, 0, 0, 0, 0, 0, 0xC4, 0x7A, 0, 0],
        )?;
        write(&ram, MARIO_STATE + 0x0C, 0x0C00_0227)?;
        write(&ram, MARIO_STATE + 0x2C, 0x0000_4000)?;
        write(&ram, MARIO_STATE + 0x90, area)?;
        write(&ram, MARIO_STATE + 0xAC, 0x0004_0880)?;
        write(&ram, area + 0x24, camera)?;
        write(&ram, camera, 0x0101_8000)?;
        let mario = mario(&ram)?;
        assert_eq!(mario.position, [1000.0, 0.0, -1000.0]);
        assert_eq!((mario.yaw, mario.health, mario.lives), (0x4000, 0x880, 4));
        assert_eq!(mario.camera_yaw, Some(i16::MIN));
        assert!(!mario.is_in_air());
        // An area pointer that is no address in RAM is no camera, and is not followed.
        write(&ram, MARIO_STATE + 0x90, 0x1234_5678)?;
        assert_eq!(super::mario(&ram)?.camera_yaw, None);
        Ok(())
    }

    #[test]
    fn the_areas_are_those_with_a_scene_and_only_the_loaded_one_places_its_nodes() -> io::Result<()>
    {
        let process = FakeProcess::new(BASE, RAM_BYTES);
        let ram = N64Ram::new(&process, BASE);
        let table = 0x8030_0000;
        write(&ram, AREAS_POINTER, table)?;
        let area = |index: u32| table + index * AREA_SIZE;
        for index in [1, 3] {
            write(&ram, area(index) + AREA_SCENE, 0x8040_0000)?;
        }
        // Area 1, loaded, has two nodes, the first with a live object; area 3 one.
        write(&ram, CURRENT_AREA_POINTER, area(1))?;
        let object = OBJECT_POOL + 5 * OBJECT_SIZE;
        write(&ram, object + OBJECT_ACTIVE_FLAGS_WORD, 0x0001_0000)?;
        ram.write(
            object + OBJECT_POS,
            &[0x42, 0xC8, 0, 0, 0, 0, 0, 0, 0x43, 0x48, 0, 0],
        )?;
        let (first, second, third) = (0x8031_0000, 0x8031_0010, 0x8031_0020);
        write(&ram, area(1) + AREA_WARP_NODES, first)?;
        ram.write(first, &[0x0A, 6, 1, 0x0B])?;
        write(&ram, first + 4, object)?;
        write(&ram, first + 8, second)?;
        ram.write(second, &[0xF0, 16, 1, 0x0A])?;
        write(&ram, area(3) + AREA_WARP_NODES, third)?;
        ram.write(third, &[0x0A, 9, 3, 0x0A])?;
        write(&ram, third + 4, object)?;

        let found = areas(&ram)?;
        let indexes: Vec<u8> = found.iter().map(|area| area.index).collect();
        assert_eq!(indexes, [1, 3]);
        let nodes = |index: usize| found.get(index).map_or(&[][..], |area| &area.nodes);
        let ends: Vec<(u8, u8, u8)> = nodes(0)
            .iter()
            .map(|node| (node.id, node.to_level, node.to_area))
            .collect();
        assert_eq!(ends, [(0x0A, 6, 1), (0xF0, 16, 1)]);
        let places: Vec<Option<[f32; 3]>> = nodes(0).iter().map(|node| node.position).collect();
        assert_eq!(places, [Some([100.0, 0.0, 200.0]), None]);
        // The same object, for an area not loaded, is no place of its.
        let elsewhere: Vec<Option<[f32; 3]>> = nodes(1).iter().map(|node| node.position).collect();
        assert_eq!(elsewhere, [None]);
        Ok(())
    }

    #[test]
    fn a_warp_and_a_level_started_afresh_leave_the_words_they_share_as_they_were() -> io::Result<()>
    {
        let process = FakeProcess::new(BASE, RAM_BYTES);
        let ram = N64Ram::new(&process, BASE);
        write(&ram, PLAY_MODE_WORD, 0x0000_7777)?;
        write(&ram, LEVEL_TO_START_WORD, 0x0000_0005)?;
        warp(&ram, 9, 2, 0x0A)?;
        assert_eq!(ram.read_u32(WARP_DEST)?, 0x0109_020A);
        assert_eq!(ram.read_u32(PLAY_MODE_WORD)?, 0x0004_7777);

        start_level(&ram, 13)?;
        assert_eq!(ram.read_u32(WARP_DEST)?, 0);
        assert_eq!(ram.read_u32(LEVEL_TO_START_WORD)?, 0x000D_0005);

        teleport(&ram, [1.0, 2.0, 3.0])?;
        assert_eq!(super::mario(&ram)?.position, [1.0, 2.0, 3.0]);
        Ok(())
    }
}
