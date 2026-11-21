#ifndef MULTIPLAYER_MAILBOX_H
#define MULTIPLAYER_MAILBOX_H

#include <PR/ultratypes.h>
#include "types.h"

// External programs read and write this RAM directly, by the numbers in this file: after
// changing it, run scripts/make_layout.py, which writes them out for the bridge, and raise
// MP_MAILBOX_VERSION. It reads each `#define MP_...` that is a plain
// number, each member's offset from the note before it, and each struct's size from its
// check at the end of this file.
//
// Range chosen by surveying a running game for RAM it never touches.
// GhostPeach.asm reads a flag at 0x807D0000, between the mailbox and the game's private data.
#define MP_MAILBOX_ADDR 0x807C0200
// The multiplayer code segment; Multiplayer/segment.asm and boot.c must agree. It began at
// 0x807C8000, and has twice been brought down into the mailbox's spare room when it filled;
// the mailbox ends 0x124 short of it.
#define MP_CODE_ADDR 0x807C5C00
#define MP_CODE_END 0x807CC000
// After the code, private object sync data (Multiplayer/objects.c); never loaded from the ROM.
#define MP_COPIES_ADDR 0x807CC000
#define MP_COPIES_END 0x807D0000
#define MP_PRIVATE_ADDR 0x807D0100
#define MP_WORLD_STASH_ADDR 0x807D0A00
#define MP_OBJECT_IDS_ADDR 0x807D0C00
// A second segment loaded from the ROM, as the first is full: the font names and chat are
// drawn in, and the display lists that go with it (Multiplayer/font.c). boot.c loads it and
// segment.asm must agree. It starts in the room the object numbers leave of theirs and runs
// into what was the front of the puppets' animation cache, which has what is left; every
// byte it takes is one the cache loses, so it holds only what the first cannot.
#define MP_CODE2_ADDR 0x807D1600
#define MP_CODE2_END 0x807D2700
// The font comes first in it. It is Multiplayer/font.bin: a byte for each cell's advance,
// then the texture.
#define MP_FONT_ADDR MP_CODE2_ADDR
#define MP_ANIM_CACHE_ADDR MP_CODE2_END
// aiseg2p1's code starts here; see AI/ai segment.asm.
#define MP_ANIM_CACHE_END 0x807E0000

#define MP_MAILBOX_MAGIC 0x42334D50 // "B3MP"
#define MP_MAILBOX_VERSION 21
#define MP_PLAYER_SLOTS 16
// Every other player in a full session. Slot 15 stays unused.
#define MP_PUPPETS 15

// One player's visible state. In `players`, external programs write it, so the game validates every
// field before using it. In `local`, the game publishes its own Mario in the same format.
//
// Writer and reader run at the same time, so `sequence` guards each slot: the writer makes it odd
// before changing the fields and even again after. A reader keeps its copy only if the sequence was
// even and unchanged around the copy.
struct MpPlayerSlot {
    /*0x00*/ u8 active;
    /*0x01*/ u8 areaIndex;
    /*0x02*/ s16 levelNum;
    /*0x04*/ Vec3f pos;
    /*0x10*/ Vec3s angle;
    /*0x16*/ s16 animID;
    /*0x18*/ s16 animFrame;
    // The numbered object Mario stands on (Multiplayer/objects.c), or 0, and where on it: his
    // place from the object's, turned back by the way the object faces. A platform is not where
    // every game has it: each runs its own, begun when that game came to the area. So a player
    // riding one is drawn on each game's own copy of it, not at `pos`, which may be in the air.
    /*0x1A*/ u16 platform;
    /*0x1C*/ Vec3s platformOffset;
    /*0x22*/ u8 reserved[0x7C - 0x22];
    /*0x7C*/ u32 sequence;
};

// Attacks that landed, as the protocol's HitKind codes. The victim's game decides their effect.
#define MP_HIT_PUNCH 1
#define MP_HIT_KICK 2
#define MP_HIT_TRIP 3
#define MP_HIT_ENTRIES 8

// One attack: in `hitsOut`, the local player hit the player in remote slot `slot`; in `hitsIn`,
// the player in remote slot `slot` hit the local player. Readers validate both fields.
struct MpHit {
    /*0x0*/ u8 slot;
    /*0x1*/ u8 kind;
    /*0x2*/ u8 pad[2];
};

// A ring with one writer, which fills entry `count % MP_HIT_ENTRIES` and then raises `count`, so
// every entry below `count` is complete. A reader keeps its own count of the entries it has taken.
struct MpHitRing {
    /*0x00*/ u32 count;
    /*0x04*/ struct MpHit entries[MP_HIT_ENTRIES];
};

// The tracker floats, in the order Multiplayer/world.c lists them.
#define MP_TRACKER_FLOATS 69

// Everything B3313 reads to generate a level, so that every player can generate the same one.
// In `localWorld`, the game publishes the values it generated with while it had no session world.
// In `sessionWorld`, the bridge writes the session's values, which the game swaps in while it
// generates a level. Guarded by `sequence` like a player slot.
struct MpWorld {
    /*0x000*/ u8 valid;
    /*0x001*/ u8 nightModeSetting;
    /*0x002*/ u16 personalizationRandSeed;
    /*0x004*/ u16 trackerFlags;
    /*0x006*/ u16 totalStars;
    /*0x008*/ u16 redStars;
    /*0x00A*/ u8 rtcHour;
    /*0x00B*/ u8 rtcMinute;
    /*0x00C*/ u8 rtcSecond;
    /*0x00D*/ u8 pad[3];
    /*0x010*/ f32 trackers[MP_TRACKER_FLOATS];
    /*0x124*/ u8 reserved[0x13C - 0x124];
    /*0x13C*/ u32 sequence;
};

// Object sync names objects by their spawn-order number (Multiplayer/objects.c), from 1 to at most
// one per pool slot. Masks hold one bit per number, bit `n % 8` of byte `n / 8`; bit 0 is unused.
#define MP_OBJECT_IDS 240
#define MP_OBJECT_MASK_BYTES 32
#define MP_OBJECTS_IN_ENTRIES 64
#define MP_INTERACTION_ENTRIES 16

// MpObjectState flags: how the owner's copy of an object is right now.
#define MP_OBJECT_INTANGIBLE (1 << 0)
#define MP_OBJECT_RENDERED (1 << 1)  // GRAPH_RENDER_ACTIVE
#define MP_OBJECT_INVISIBLE (1 << 2) // GRAPH_RENDER_INVISIBLE
// The object has spawned enemies that are still about, so copies keep running their own behavior
// to spawn theirs, and follow only where the owner's is and how it looks.
#define MP_OBJECT_PARENT (1 << 3)
// The upper half of the flags: for a spawned enemy, the kind games make for themselves that it
// is, from 1 (Multiplayer/objects.c), or 0.
#define MP_OBJECT_KIND_SHIFT 4
#define MP_OBJECT_KIND_MASK 0xF
// In a spawner's state, where there is no kind: every enemy it has spawned and still has is of
// a kind games make for themselves. Its copies then do as it does and do not run: run, each
// would decide by its own Mario whether its enemies are loaded, and an enemy made there from
// the owner's state would be unloaded again by a spawner whose Mario is far off.
#define MP_OBJECT_REPLICATES (1 << MP_OBJECT_KIND_SHIFT)

// One synced object's state, which its owner's game publishes and every other game mirrors. The
// part from `id` on is what travels over the network; the bridge fills in the level and area.
struct MpObjectState {
    /*0x00*/ s16 levelNum;
    /*0x02*/ u8 areaIndex;
    /*0x03*/ u8 pad;
    /*0x04*/ u16 id;
    /*0x06*/ u16 action;
    /*0x08*/ Vec3f pos;
    /*0x14*/ f32 velY;
    /*0x18*/ f32 forwardVel;
    /*0x1C*/ s16 faceYaw;
    /*0x1E*/ s16 moveYaw;
    /*0x20*/ s16 facePitch;
    /*0x22*/ s16 faceRoll;
    /*0x24*/ u16 timer;
    /*0x26*/ u8 subAction;
    /*0x27*/ s8 health;
    /*0x28*/ s16 scale[3]; // In 256ths.
    /*0x2E*/ s8 animIndex; // Into the object's own oAnimations, or -1.
    /*0x2F*/ u8 opacity;
    /*0x30*/ s16 animFrame;
    /*0x32*/ u8 flags;
    /*0x33*/ u8 check; // A byte of the object's behavior: a state is only for one of the same.
    /*0x34*/ u32 behParams; // oBehParams: what a spawner remembers, and what a spawned enemy is.
};

// A claim on a copy of an object another game runs, which the local Mario attacked or picked up,
// with the oInteractStatus the touch gave it: the game runs the object itself from then on, and
// the server makes it the owner. Readers validate both fields.
struct MpInteraction {
    /*0x0*/ s16 levelNum;
    /*0x2*/ u8 areaIndex;
    /*0x3*/ u8 pad;
    /*0x4*/ u16 id;
    /*0x6*/ u16 pad2;
    /*0x8*/ u32 status;
};

// A ring like MpHitRing.
struct MpInteractionRing {
    /*0x00*/ u32 count;
    /*0x04*/ struct MpInteraction entries[MP_INTERACTION_ENTRIES];
};

// Spawned enemies gone from the game that runs them, by name (Multiplayer/objects.c), as a ring
// like MpHitRing: in `childrenGoneOut` the game reports its own, so the server hears at once.
// Readers validate the names.
#define MP_NAME_ENTRIES 32
struct MpNameRing {
    /*0x00*/ u32 count;
    /*0x04*/ u16 names[MP_NAME_ENTRIES];
};

// The spawned enemies alive in an area, by name, as the games that run them last said: the
// server's whole list, which the bridge writes when it changes and now and then. A game removes
// its copy of another game's enemy that is not in it. Being the whole list, one that is lost on
// the way is made good by the next, where word of each enemy's going could be lost for good.
// `count` is MP_LIVE_UNKNOWN when there are too many to list. Guarded by `sequence` like a
// player slot.
#define MP_LIVE_CHILDREN 48
#define MP_LIVE_UNKNOWN 0xFF
struct MpLiveChildren {
    /*0x00*/ s16 levelNum;
    /*0x02*/ u8 areaIndex;
    /*0x03*/ u8 count;
    /*0x04*/ u16 names[MP_LIVE_CHILDREN];
    /*0x64*/ u32 sequence;
};

// Which objects of an area the local game runs for everyone, and which their owners have reported
// gone. Written by the bridge from the server's word, guarded by `sequence` like a player slot.
struct MpObjectsOwned {
    /*0x00*/ s16 levelNum;
    /*0x02*/ u8 areaIndex;
    /*0x03*/ u8 pad;
    /*0x04*/ u8 owned[MP_OBJECT_MASK_BYTES];
    /*0x24*/ u8 gone[MP_OBJECT_MASK_BYTES];
    /*0x44*/ u8 reserved[0x4C - 0x44];
    /*0x4C*/ u32 sequence;
};

// States from other objects' owners, as a ring like MpHitRing. The game applies each entry once.
struct MpObjectsIn {
    /*0x000*/ u32 count;
    /*0x004*/ struct MpObjectState entries[MP_OBJECTS_IN_ENTRIES];
};

// What the local game publishes each frame: the state of every synced object it owns, and which
// synced objects of the area are gone here. Guarded by `sequence`, which comes first because the
// bridge reads only the first `count` states.
struct MpObjectsOut {
    /*0x00*/ u32 sequence;
    /*0x04*/ s16 levelNum;
    /*0x06*/ u8 areaIndex;
    /*0x07*/ u8 pad;
    /*0x08*/ u16 count;
    /*0x0A*/ u16 pad2;
    /*0x0C*/ u8 gone[MP_OBJECT_MASK_BYTES];
    /*0x2C*/ struct MpObjectState states[MP_OBJECT_IDS];
};

// The bytes of a save file that hold stars: courseStars through extraStars. B3313's later courses
// run on past courseStars into courseCoinScores, which it never uses for coins.
#define MP_STAR_BYTES 58

// A save file's progress, which players share (Multiplayer/progress.c). In `progressOut`, the game
// publishes its current file's; in `progressIn`, the bridge writes the session's, which the game
// adds to its file. The part through `betaKeys` travels over the network. Guarded by `sequence`
// like a player slot.
struct MpProgress {
    /*0x00*/ u32 flags; // Save flags shared by everyone; see MP_SHARED_SAVE_FLAGS.
    /*0x04*/ u8 stars[MP_STAR_BYTES];
    /*0x3E*/ u16 keyFlags;
    /*0x40*/ u16 betaKeys; // B3313's beta keys, which it keeps outside the save file.
    /*0x42*/ u8 saveFile;  // In `progressOut`: the file played, 1 to 4, or 0 outside a level.
    /*0x43*/ u8 pad;
    /*0x44*/ u32 sequence;
};

// Coins collected, which players share (Multiplayer/objects.c): in `coinsOut` the local Mario's, in
// `coinsIn` other players' in this level. `name` is the coin's, or 0 if it has none. In `coinsIn`,
// a coin worth nothing is one collected before this game came to the area: it goes, uncounted.
// Readers validate every field.
#define MP_COIN_ENTRIES 16
struct MpCoin {
    /*0x0*/ s16 levelNum;
    /*0x2*/ u8 areaIndex;
    /*0x3*/ u8 value;
    /*0x4*/ u16 name;
    /*0x6*/ u16 pad;
};

// A ring like MpHitRing.
struct MpCoinRing {
    /*0x00*/ u32 count;
    /*0x04*/ struct MpCoin entries[MP_COIN_ENTRIES];
};

// Text the bridge writes is plain characters, the codes 0x20 to 0x7E, ending in a zero, as
// memory nobody has written does. The game draws it in its own font (Multiplayer/people.c),
// after replacing any code that font has no glyph for.
#define MP_TEXT_END 0
// A name: up to 16 characters and the end code, padded.
#define MP_NAME_BYTES 20
// What a player may do, as the protocol's Role codes.
#define MP_ROLE_PLAYER 0
#define MP_ROLE_MODERATOR 1
#define MP_ROLE_HOST 2
// A player's byte in MpPeople holds more than the role, in its low two bits: MP_PERSON_DRESSED
// says the high four bits are the outfit they chose, a row of sPlayerColors in puppet.c. It
// is clear in a slot nobody is in and outside a session, where Mario wears the game's own.
#define MP_ROLE_MASK 0x03
#define MP_PERSON_DRESSED 0x08
#define MP_PERSON_OUTFIT_SHIFT 4

// Who the players are: the local player's name and role, and those of the player in each remote
// slot, in the slots' order. Written by the bridge, guarded by `sequence` like a player slot.
struct MpPeople {
    /*0x000*/ u8 localRole;
    /*0x001*/ u8 roles[MP_PLAYER_SLOTS];
    /*0x011*/ u8 pad[3];
    /*0x014*/ u8 localName[MP_NAME_BYTES];
    /*0x028*/ u8 names[MP_PLAYER_SLOTS][MP_NAME_BYTES];
    /*0x168*/ u32 sequence;
};

// The chat as the game shows it: the bridge cuts each line into rows short enough for the
// chat's corner of the screen and writes them into a ring like MpHitRing. `shown` is how many
// of the newest rows to draw now, which the bridge sets when a row arrives and clears a few
// seconds later: the game cannot time that, as its own rate varies with the emulator.
//
// The player types a line on the PC's keyboard, which the bridge reads, as the game has none.
// While they do, `draft` is that line as it stands, with its prompt and cursor, and `typing`
// is not 0: the game draws the chat as a box with the draft under its rows, and keeps the
// controller from Mario (mp_read_pads in Multiplayer/people.c). `typing` counts frames, as
// `bridgeLease` does: the bridge sets it to MP_TYPING_LEASE again and again and the game
// counts it down, so that a bridge that was killed does not leave the controls dead.
#define MP_CHAT_ROWS 8
// A row: up to 51 characters and the end code. The bridge cuts rows by their width in the
// font, to MP_CHAT_ROW_PIXELS at most, which the chat's box is made as wide as.
#define MP_CHAT_ROW_BYTES 52
#define MP_CHAT_ROW_PIXELS 200
#define MP_CHAT_SHOWN_MOST 6
// About a second of play.
#define MP_TYPING_LEASE 30
struct MpChat {
    /*0x000*/ u32 count;
    /*0x004*/ u8 shown;
    /*0x005*/ u8 typing;
    /*0x006*/ u8 pad[2];
    /*0x008*/ u8 rows[MP_CHAT_ROWS][MP_CHAT_ROW_BYTES];
    /*0x1A8*/ u8 draft[MP_CHAT_ROW_BYTES];
};

// How many frames of the game a bridge's word that it is here lasts: about five seconds of
// play, and two in the test emulators, which run faster. The game cannot time it otherwise.
#define MP_BRIDGE_LEASE 150
// In a record about a place: no level, so the game, which is always in one, ignores the record.
#define MP_NO_LEVEL (-1)

struct MpMailbox {
    /*0x0000*/ u32 magic;
    /*0x0004*/ u32 version;
    // Frames left before the game takes its bridge for gone. The bridge sets it to
    // MP_BRIDGE_LEASE each time it draws the other players, and the game counts it down each
    // frame, paused or not; at 0 it removes what the session left for it to act on (mp_read_pads),
    // as a bridge does when it leaves, since one that was killed could not. 0 before any bridge.
    /*0x0008*/ u32 bridgeLease;
    // Which version of B3313 this is, by its number in scripts/game_versions.py: players can
    // share a session only if their games are the same. Set at boot, with the magic.
    /*0x000C*/ u32 game;
    /*0x0010*/ struct MpHitRing hitsOut; // Written by the game.
    /*0x0034*/ u8 reserved1[0x040 - 0x034];
    /*0x0040*/ struct MpHitRing hitsIn; // Written by the bridge.
    /*0x0064*/ u8 reserved3[0x100 - 0x064];
    /*0x0100*/ struct MpPlayerSlot local;
    /*0x0180*/ u8 reserved2[0x200 - 0x180];
    /*0x0200*/ struct MpPlayerSlot players[MP_PLAYER_SLOTS];
    /*0x0A00*/ struct MpWorld localWorld;
    /*0x0B40*/ struct MpWorld sessionWorld;
    /*0x0C80*/ struct MpObjectsOwned objectsOwned; // Written by the bridge.
    /*0x0CD0*/ u8 reserved4[0xD00 - 0xCD0];
    /*0x0D00*/ struct MpObjectsIn objectsIn; // Written by the bridge.
    /*0x1B04*/ u8 reserved5[0x1B80 - 0x1B04];
    /*0x1B80*/ struct MpInteractionRing interactionsOut; // Written by the game.
    /*0x1C44*/ u8 reserved6[0x1C80 - 0x1C44];
    /*0x1C80*/ struct MpNameRing childrenGoneOut; // Written by the game.
    /*0x1CC4*/ u8 reserved8[0x1D00 - 0x1CC4];
    /*0x1D00*/ struct MpLiveChildren liveChildren; // Written by the bridge.
    /*0x1D68*/ u8 reserved9[0x1D80 - 0x1D68];
    /*0x1D80*/ struct MpObjectsOut objectsOut; // Written by the game.
    /*0x522C*/ u8 reserved10[0x5280 - 0x522C];
    /*0x5280*/ struct MpProgress progressOut; // Written by the game.
    /*0x52C8*/ u8 reserved11[0x5300 - 0x52C8];
    /*0x5300*/ struct MpProgress progressIn; // Written by the bridge.
    /*0x5348*/ u8 reserved12[0x5380 - 0x5348];
    /*0x5380*/ struct MpCoinRing coinsOut; // Written by the game.
    /*0x5404*/ u8 reserved13[0x5480 - 0x5404];
    /*0x5480*/ struct MpCoinRing coinsIn; // Written by the bridge.
    // How many entries of `coinsIn` the game has taken, which it writes each frame it runs:
    // the bridge adds no more than the ring holds beyond it, so a level's worth of coins sent
    // at once, to a game that has just come there, is not lost.
    /*0x5504*/ u32 coinsInTaken;
    /*0x5508*/ u8 reserved14[0x5580 - 0x5508];
    /*0x5580*/ struct MpPeople people; // Written by the bridge.
    /*0x56EC*/ u8 reserved15[0x5700 - 0x56EC];
    /*0x5700*/ struct MpChat chat; // Written by the bridge.
};

// Compile-time size checks: a negative array size fails the build.
typedef char MpPlayerSlotSizeCheck[sizeof(struct MpPlayerSlot) == 0x80 ? 1 : -1];
typedef char MpWorldSizeCheck[sizeof(struct MpWorld) == 0x140 ? 1 : -1];
typedef char MpHitSizeCheck[sizeof(struct MpHit) == 0x4 ? 1 : -1];
typedef char MpInteractionSizeCheck[sizeof(struct MpInteraction) == 0xC ? 1 : -1];
typedef char MpCoinSizeCheck[sizeof(struct MpCoin) == 0x8 ? 1 : -1];
typedef char MpHitRingSizeCheck[sizeof(struct MpHitRing) == 0x24 ? 1 : -1];
typedef char MpObjectStateSizeCheck[sizeof(struct MpObjectState) == 0x38 ? 1 : -1];
typedef char MpInteractionRingSizeCheck[sizeof(struct MpInteractionRing) == 0xC4 ? 1 : -1];
typedef char MpObjectsOwnedSizeCheck[sizeof(struct MpObjectsOwned) == 0x50 ? 1 : -1];
typedef char MpObjectsInSizeCheck[sizeof(struct MpObjectsIn) == 0xE04 ? 1 : -1];
typedef char MpObjectsOutSizeCheck[sizeof(struct MpObjectsOut) == 0x34AC ? 1 : -1];
typedef char MpLiveChildrenSizeCheck[sizeof(struct MpLiveChildren) == 0x68 ? 1 : -1];
typedef char MpNameRingSizeCheck[sizeof(struct MpNameRing) == 0x44 ? 1 : -1];
typedef char MpProgressSizeCheck[sizeof(struct MpProgress) == 0x48 ? 1 : -1];
typedef char MpCoinRingSizeCheck[sizeof(struct MpCoinRing) == 0x84 ? 1 : -1];
typedef char MpPeopleSizeCheck[sizeof(struct MpPeople) == 0x16C ? 1 : -1];
typedef char MpChatSizeCheck[sizeof(struct MpChat) == 0x1DC ? 1 : -1];
typedef char MpMailboxSizeCheck[sizeof(struct MpMailbox) == 0x58DC ? 1 : -1];
// The mailbox must end before the multiplayer code, which ends before GhostPeach.asm's flag at
// 0x807D0000.
typedef char MpMailboxEndCheck[MP_MAILBOX_ADDR + sizeof(struct MpMailbox) <= MP_CODE_ADDR ? 1 : -1];
typedef char MpCodeEndCheck[MP_CODE_END <= MP_COPIES_ADDR && MP_COPIES_END <= 0x807D0000 ? 1 : -1];

#define gMpMailbox ((struct MpMailbox *) MP_MAILBOX_ADDR)

#endif
