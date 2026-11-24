#include "sm64.h"
#include "area.h"
#include "engine/graph_node.h"
#include "engine/math_util.h"
#include "interaction.h"
#include "level_update.h"
#include "object_constants.h"
#include "object_fields.h"
#include "object_helpers.h"
#include "object_list_processor.h"
#include "behavior_data.h"
#include "game/memory.h"
#include "model_ids.h"

#include "Multiplayer/mailbox.h"
#include "Multiplayer/objects.h"

// Object sync needs every game to name each object the same way. Pool slots cannot, since puppets
// and runtime spawns take them at different times in different games, and an object's behavior,
// parameters and home are shared by many objects. Spawn order can: with the session's world
// inputs, an area's objects spawn in the same order in every game, and each object list keeps its
// objects in the order they were created. So once an area has loaded, walking the lists in a fixed
// order numbers its objects the same everywhere.
//
// Each synced object is run by one game, its owner's. In every other game it is a copy: its
// behavior does not run, and it mirrors the owner's latest state instead, as puppets mirror other
// players. Its own behavior would chase its own game's Mario between states, which drags it away
// from where the owner has it. A local Mario still touches a copy; when he attacks one or picks it
// up, this game claims it: it runs the object itself from that frame, so the attack lands at once
// and as he made it, and the server, told of the claim, makes this game its owner.

extern const BehaviorScript bhvMpPuppet[];
extern void cur_obj_update(void);
extern void cur_obj_init_animation(s32 animIndex);
// Multiplayer/puppet.c: the puppet in remote slot `slot`, if that player is drawn here.
extern struct Object *mp_visible_puppet(s32 slot);
// Checks the free object list first, where spawn_object hangs; see Multiplayer/segment.asm.
extern struct Object *try_to_spawn_object(s16 offsetY, f32 scale, struct Object *parent, s32 model,
                                          const BehaviorScript *behavior);

#define NO_SLOT 0xFF

// The kinds of spawned enemy a game makes for itself when another game's spawner has spawned one
// and its own has not, as when its Mario is too far from the spawner: without this, a player
// would not see the enemies another is fighting. A state names its object's kind by its place
// here, from 1, never by an address; an enemy of any other kind is still only in the games whose
// own spawners spawned it.
//
// Only kinds whose objects are in a synced list (is_synced_list) belong here: spawned enemies
// are named from those lists alone, and one made here that is never named is made again for
// every state that comes for it. A bob-omb is in the game's list of destructive objects, so it
// is not one.
struct MpSpawnKind {
    const BehaviorScript *behavior;
    s32 model;
};

static const struct MpSpawnKind sSpawnKinds[] = {
    { bhvGoomba, MODEL_GOOMBA },
};
#define SPAWN_KINDS ((s32) (sizeof(sSpawnKinds) / sizeof(sSpawnKinds[0])))
typedef char MpSpawnKindsCheck[SPAWN_KINDS <= MP_OBJECT_KIND_MASK ? 1 : -1];

// A spawned enemy another game runs goes from this game when the server's list of those alive
// no longer has it, unless it is newer than this many frames, or a state for it came within
// this many: its owner's first word of it may still be on the way.
#define CHILD_GRACE_FRAMES 60
#define STATE_GRACE_FRAMES 20
// What is kept of each pool slot's object in MpCopies.slotFlags.
#define SLOT_COIN_TOLD (1 << 0)   // Its collecting has been reported or taken, so it is told once.
#define SLOT_NET_SPAWNED (1 << 1) // Made here for another game's spawner, not by this game's own.

// A copy mirrors its owner while states keep coming: the owner refreshes each object about once a
// second, so a copy that hears nothing for longer runs on its own again.
#define FOLLOW_FRAMES 240
// Another player must be this many times nearer an object than the local one, by the square of
// the distance, to be the one it sees: about 0.7 of the distance, so it does not turn from one
// to the other as two players stand about it.
#define NEARER_BY 2.0f
// How long a game runs an object it has claimed while it waits to hear it is the owner; if the
// server refuses the claim, the object is a copy again after this.
#define CLAIM_FRAMES 200
// An object's animations are a list in its oAnimations; indices beyond this are not looked up.
#define MAX_ANIM_INDEX 32
#define SCALE_ONE 256.0f
// What an interaction may tell an object's owner: that it was touched, attacked, and how.
#define FORWARDED_STATUS (INT_STATUS_INTERACTED | INT_STATUS_WAS_ATTACKED | INT_STATUS_ATTACK_MASK)
#define RAM_START 0x80000000
#define RAM_END 0x80800000

// How a numbered object is shared. A mirrored object's copies show what its owner's game does. A
// reported one, a spawner, runs in every game, so each spawns its own enemies, and its owner's game
// only reports where it is: whoever runs a spawner runs the enemies it spawns.
#define SYNC_NONE 0
#define SYNC_MIRRORED 1
#define SYNC_REPORTED 2

// An enemy that a numbered object spawned is named for its spawner and itself: the high bit, the
// spawner's number, and seven bits of its behavior and parameters, which the same spawner gives the
// same child in every game. The protocol reads names the same way.
#define CHILD_NAME_BIT 0x8000
#define CHILD_KEY_BITS 7
#define CHILD_KEY_MASK ((1 << CHILD_KEY_BITS) - 1)

// One entry per pool slot: the object's number in its area, or 0, and the behavior it had, so a
// recycled slot is not taken for the object that was numbered there.
struct MpObjectId {
    u16 id;
    u8 synced; // SYNC_NONE, SYNC_MIRRORED or SYNC_REPORTED.
    u8 left; // Whether the object numbered has gone from the slot; see note_slots_left.
    const BehaviorScript *behavior;
};

// How many coins this game may owe at once: more than a ring's or a line's, and the rest, if
// an area ever had more, are not taken.
#define OWED_COINS 32

struct MpObjectIds {
    s16 levelNum;
    u8 areaIndex;
    u8 pad;
    u16 count; // Objects numbered when the area loaded.
    u16 pad2;
    u32 objectsInTaken; // Entries of the mailbox's `objectsIn` ring taken so far.
    // Whether the area was loaded since the server's word on what is gone from it was last
    // acted on; see remove_gone_objects.
    u32 goneUnchecked;
    struct MpObjectId slots[OBJECT_POOL_CAPACITY];
    u8 slotOfId[MP_OBJECT_IDS + 1]; // Each number's pool slot, or NO_SLOT.
    // For the coin in each slot that a defeated enemy dropped, the number of the object behind
    // that enemy, itself or its spawner; 0 for anything else. See note_loot.
    u8 lootSpawner[OBJECT_POOL_CAPACITY];
    // Names of coins collected in this area before this game came that it had no coin for
    // when it was told, each once for each such coin, and 0 for a place not in use: a ring of
    // coins has none until a player comes near it. See take_owed_coins.
    u16 owedCoins[OWED_COINS];
};

// In the zeroed RAM before the second segment, where tools that test a running game read it.
#define sIds ((struct MpObjectIds *) MP_OBJECT_IDS_ADDR)
typedef char MpObjectIdsSizeCheck[sizeof(struct MpObjectIds) <= MP_CODE2_ADDR - MP_OBJECT_IDS_ADDR
                                      ? 1
                                      : -1];
// A number never exceeds the pool, so every one fits the masks and the out block.
typedef char MpObjectIdsFitCheck[OBJECT_POOL_CAPACITY <= MP_OBJECT_IDS
                                         && MP_OBJECT_IDS < MP_OBJECT_MASK_BYTES * 8
                                     ? 1
                                     : -1];

// What a copy mirrors: its owner's latest state for the object in this slot, and how long ago it
// came.
struct MpCopy {
    u16 id; // The object the state is about, or 0.
    u8 framesSinceState;
    u8 claim; // Frames left of this game's claim on the object in this slot, or 0.
    struct MpObjectState target;
};

struct MpCopies {
    struct MpObjectsOwned owned; // This frame's word on ownership, valid if `knowsOwners`.
    s32 knowsOwners;
    u32 claimsMade; // For tools that test a running game.
    struct MpCopy slots[OBJECT_POOL_CAPACITY];
    u16 childName[OBJECT_POOL_CAPACITY]; // This frame's name of the spawned enemy in each slot, or 0.
    u16 ownedChildLast[OBJECT_POOL_CAPACITY]; // Last frame's, for those this game ran.
    // name_children's working space, here rather than on the game thread's small stack.
    u8 foundSlots[OBJECT_POOL_CAPACITY];
    u8 slotFlags[OBJECT_POOL_CAPACITY]; // SLOT_* of the object in each slot.
    // How many frames the spawned enemy in each slot has had its name, up to 255.
    u8 childAge[OBJECT_POOL_CAPACITY];
    u32 coinsInTaken; // Entries of the mailbox's `coinsIn` ring taken so far.
    // The server's list of the spawned enemies alive in this area, if `liveKnown`.
    u16 liveNames[MP_LIVE_CHILDREN];
    u8 liveCount;
    u8 liveKnown;
};
// framesSinceState counts up to FOLLOW_FRAMES and a claim down from CLAIM_FRAMES, in a byte each.
typedef char MpCopyCountsCheck[FOLLOW_FRAMES <= 255 && CLAIM_FRAMES <= 255 ? 1 : -1];

#define sCopies ((struct MpCopies *) MP_COPIES_ADDR)
typedef char MpCopiesSizeCheck[sizeof(struct MpCopies) <= MP_COPIES_END - MP_COPIES_ADDR ? 1 : -1];
// Tools that test a running game read these two structs by their fields' places, which they
// write out: a change here that moves one must be made there too, and fails here until it is.
#define PLACE_OF(type, member) ((u32) & (((type *) 0)->member))
typedef char MpCopiesPlacesCheck[sizeof(struct MpCopy) == 60
                                         && PLACE_OF(struct MpCopies, claimsMade) == 0x54
                                         && PLACE_OF(struct MpCopies, slots) == 0x58
                                         && PLACE_OF(struct MpCopies, childName)
                                                == 0x58 + OBJECT_POOL_CAPACITY * 60
                                     ? 1
                                     : -1];
typedef char MpObjectIdsPlacesCheck[sizeof(struct MpObjectId) == 8
                                            && PLACE_OF(struct MpObjectIds, slots) == 16
                                            && PLACE_OF(struct MpObjectIds, lootSpawner)
                                                   == 16 + OBJECT_POOL_CAPACITY * 8
                                                          + MP_OBJECT_IDS + 1
                                        ? 1
                                        : -1];

static void take_coins(void);
static s32 is_coin(struct Object *obj);
static void drop_loot(struct Object *obj, u32 name);

static s32 slot_of(struct Object *obj) {
    return obj - gObjectPool;
}

// Only what the area itself placed is numbered: those objects are their own parents. Mario and
// the puppets are left out, since they arrive at different times in different games.
static s32 is_numbered_kind(struct Object *obj) {
    return obj->parentObj == obj && obj != gMarioObject && obj->behavior != bhvMpPuppet;
}

// Enemies, for now: the game's general and pushable actors. Others, such as platforms, coins and
// stars, run the same in every game or wait for shared progress.
static s32 is_synced_list(s32 list) {
    return list == OBJ_LIST_GENACTOR || list == OBJ_LIST_PUSHABLE;
}

// How an object of list `list` is shared. One without a model in an enemy list, such as a goomba
// spawner, which vanilla puts in the pushable list, has nothing to show; its copies must still run,
// or they would never spawn, and their enemies would exist only in its owner's game.
static u8 sync_kind(struct Object *obj, s32 list) {
    if (list == OBJ_LIST_SPAWNER) {
        return SYNC_REPORTED;
    }
    if (!is_synced_list(list)) {
        return SYNC_NONE;
    }
    return obj->header.gfx.sharedChild != NULL ? SYNC_MIRRORED : SYNC_REPORTED;
}

void mp_number_area_objects(void) {
    struct ObjectNode *list;
    struct ObjectNode *node;
    struct Object *obj;
    s32 slot;
    s32 i;
    u16 next = 0;

    for (i = 0; i < OBJECT_POOL_CAPACITY; i++) {
        sIds->slots[i].id = 0;
        sIds->slots[i].synced = SYNC_NONE;
        sIds->slots[i].left = FALSE;
        sIds->slots[i].behavior = NULL;
        sCopies->slots[i].id = 0;
        sCopies->slots[i].claim = 0;
        // Names from the area left behind name nothing here, and are not gone from it.
        sCopies->childName[i] = 0;
        sCopies->ownedChildLast[i] = 0;
        sCopies->slotFlags[i] = 0;
        sCopies->childAge[i] = 0;
    }
    for (i = 0; i <= MP_OBJECT_IDS; i++) {
        sIds->slotOfId[i] = NO_SLOT;
    }
    for (i = 0; i < NUM_OBJ_LISTS; i++) {
        list = &gObjectLists[i];
        for (node = list->next; node != list; node = node->next) {
            obj = (struct Object *) node;
            slot = slot_of(obj);
            if (slot < 0 || slot >= OBJECT_POOL_CAPACITY || !is_numbered_kind(obj)) {
                continue;
            }
            next++;
            sIds->slots[slot].id = next;
            sIds->slots[slot].synced = sync_kind(obj, i);
            sIds->slots[slot].behavior = obj->behavior;
            sIds->slotOfId[next] = slot;
        }
    }
    sIds->levelNum = gCurrLevelNum;
    sIds->areaIndex = gCurrAreaIndex;
    sIds->count = next;
    sIds->goneUnchecked = TRUE;
    // What was owed is among the coins the game is told of again, below.
    for (i = 0; i < OWED_COINS; i++) {
        sIds->owedCoins[i] = 0;
    }
    // The area's coins are all back, the ones others collected among them: the bridge has the
    // game told again which those are.
    gMpMailbox->areaLoads++;
}

u16 mp_object_id(struct Object *obj) {
    s32 slot = slot_of(obj);
    struct MpObjectId *entry;

    if (slot < 0 || slot >= OBJECT_POOL_CAPACITY) {
        return 0;
    }
    entry = &sIds->slots[slot];
    // A slot freed and filled again holds a different object, spawned at runtime by another
    // object, so its parent is not itself.
    if (entry->id == 0 || obj->activeFlags == ACTIVE_FLAG_DEACTIVATED
        || obj->behavior != entry->behavior || obj->parentObj != obj) {
        return 0;
    }
    return entry->id;
}

struct Object *mp_numbered_object(u32 id) {
    u8 slot;

    if (id == 0 || id > sIds->count || sIds->levelNum != gCurrLevelNum
        || sIds->areaIndex != gCurrAreaIndex) {
        return NULL;
    }
    slot = sIds->slotOfId[id];
    if (slot >= OBJECT_POOL_CAPACITY || mp_object_id(&gObjectPool[slot]) != id) {
        return NULL;
    }
    return &gObjectPool[slot];
}

// Notes each numbered slot whose object is gone, before anything can be spawned into it: the
// game frees an object at the end of the frame it goes in, and this runs first in the next.
// A coin spawned there later is then not taken for the one that was numbered.
static void note_slots_left(void) {
    struct MpObjectId *entry;
    struct Object *obj;
    s32 slot;

    for (slot = 0; slot < OBJECT_POOL_CAPACITY; slot++) {
        entry = &sIds->slots[slot];
        obj = &gObjectPool[slot];
        if (entry->id != 0 && (obj->activeFlags == ACTIVE_FLAG_DEACTIVATED
                               || obj->behavior != entry->behavior)) {
            entry->left = TRUE;
        }
    }
}

// The number `obj` was given when the area loaded, if it is still the object that was numbered,
// whatever its parent is now: unlike mp_object_id, which takes an object that is no longer its
// own parent for a later one spawned into the slot. Red coins make the red coin star their
// parent, and would all get one name from it, where each has its own number.
static u16 number_at_load(struct Object *obj) {
    s32 slot = slot_of(obj);

    if (slot < 0 || slot >= OBJECT_POOL_CAPACITY || sIds->slots[slot].left
        || obj->activeFlags == ACTIVE_FLAG_DEACTIVATED
        || obj->behavior != sIds->slots[slot].behavior || sIds->levelNum != gCurrLevelNum
        || sIds->areaIndex != gCurrAreaIndex) {
        return 0;
    }
    return sIds->slots[slot].id;
}

// The pool slot that number `id` was given to in the current area, if it is shared at all, or
// NO_SLOT. `id` may come from the network, so it is range-checked before it indexes anything.
static u8 synced_slot(u32 id) {
    u8 slot;

    if (id == 0 || id > sIds->count || sIds->levelNum != gCurrLevelNum
        || sIds->areaIndex != gCurrAreaIndex) {
        return NO_SLOT;
    }
    slot = sIds->slotOfId[id];
    if (slot >= OBJECT_POOL_CAPACITY || sIds->slots[slot].synced == SYNC_NONE) {
        return NO_SLOT;
    }
    return slot;
}

static s32 is_child_name(u32 name) {
    return (name & CHILD_NAME_BIT) != 0;
}

// The number of the object that a spawned enemy's name names it for, or of the object itself.
static u32 owning_number(u32 name) {
    return is_child_name(name) ? (name >> CHILD_KEY_BITS) & 0xFF : name;
}

// The name of the object in `slot`, if it is shared: its number if it is mirrored, or its name if
// it is a spawned enemy; otherwise 0. Spawners are reported, not mirrored, so they have none.
static u16 name_of(s32 slot) {
    u16 id = mp_object_id(&gObjectPool[slot]);

    if (id != 0) {
        return sIds->slots[slot].synced == SYNC_MIRRORED ? id : 0;
    }
    return sCopies->childName[slot];
}

// The slot of the shared object named `name` in the current area, if it is still here, or NO_SLOT.
// `name` may come from the network: it only ever matches names this game gave its own objects.
static u8 named_slot(u32 name) {
    s32 slot;

    if (name == 0) {
        return NO_SLOT;
    }
    if (!is_child_name(name)) {
        slot = synced_slot(name);
        return slot != NO_SLOT && name_of(slot) == name ? slot : NO_SLOT;
    }
    for (slot = 0; slot < OBJECT_POOL_CAPACITY; slot++) {
        if (sCopies->childName[slot] == name) {
            return slot;
        }
    }
    return NO_SLOT;
}

// The shared object named `name` in the current area, if it is still here.
static struct Object *named_object(u32 name) {
    u8 slot = named_slot(name);

    return slot == NO_SLOT ? NULL : &gObjectPool[slot];
}

// The synced object numbered `id` in the current area, mirrored or reported, if it is still here.
static struct Object *synced_object(u32 id) {
    u8 slot = synced_slot(id);

    if (slot == NO_SLOT || mp_object_id(&gObjectPool[slot]) != id) {
        return NULL;
    }
    return &gObjectPool[slot];
}

static s32 mask_has(volatile u8 *mask, u32 id) {
    return (mask[id / 8] >> (id % 8)) & 1;
}

// A coin a defeated enemy dropped is as old as this, at most, when it is first seen here.
#define LOOT_NEW_FRAMES 2

// Notes, for each coin a defeated enemy has just dropped, which numbered object was behind that
// enemy, so the coin can be named: by the time it is collected its parent is long gone, and its
// slot may hold something else. Every game drops such coins for itself, the owner's by the
// enemy's own behavior and the others' by drop_loot, and names them alike, so one collected in
// any game goes from all. Runs first in a frame, while the names of the frame before still
// stand and the enemy's slot, freed at that frame's end, cannot have been filled again.
static void note_loot(void) {
    struct MpObjectId *entry;
    struct Object *obj;
    s32 parent;
    s32 slot;

    for (slot = 0; slot < OBJECT_POOL_CAPACITY; slot++) {
        obj = &gObjectPool[slot];
        if (!is_coin(obj) || sIds->levelNum != gCurrLevelNum || sIds->areaIndex != gCurrAreaIndex) {
            sIds->lootSpawner[slot] = 0;
            continue;
        }
        parent = slot_of(obj->parentObj);
        if (sIds->lootSpawner[slot] != 0 || obj->oTimer > LOOT_NEW_FRAMES || obj->parentObj == obj
            || parent < 0 || parent >= OBJECT_POOL_CAPACITY
            || obj->parentObj->activeFlags != ACTIVE_FLAG_DEACTIVATED) {
            continue;
        }
        entry = &sIds->slots[parent];
        sIds->lootSpawner[slot] = entry->id != 0 && !entry->left
            ? entry->id
            : owning_number(sCopies->childName[parent]);
    }
}

// Whether another game runs the object named `name`, so this game's is a copy. A spawned enemy is
// run by whoever runs its spawner.
static s32 is_others(u32 name) {
    u32 number = owning_number(name);

    // A name from the network may be any number: one past the mask is nobody's.
    return sCopies->knowsOwners && number <= MP_OBJECT_IDS
        && !mask_has(sCopies->owned.owned, number);
}

// Whether this game runs the object named `name`, in `slot`: it owns it, or has claimed it.
static s32 runs_here(u32 name, s32 slot) {
    return !is_others(name) || sCopies->slots[slot].claim != 0;
}

// A byte of an object's behavior, which a state carries: a game takes a state only for an object
// of the same behavior. Numbers come from the order an area's objects spawned in, and a game
// whose area spawned one fewer, as after its player took a 1-up there, numbers what follows one
// lower; this keeps such a state off the wrong object.
static u8 behavior_check(struct Object *obj) {
    u32 behavior = (u32) obj->behavior;

    return (behavior >> 2) ^ (behavior >> 10) ^ (behavior >> 18);
}

// The kind of spawned enemy `obj` is, from 1, or 0 if games do not make its kind for themselves.
static s32 spawn_kind(struct Object *obj) {
    s32 i;

    for (i = 0; i < SPAWN_KINDS; i++) {
        if (segmented_to_virtual(sSpawnKinds[i].behavior) == obj->behavior) {
            return i + 1;
        }
    }
    return 0;
}

// Seven bits of what makes a spawned enemy itself: its behavior, the same in every game, and the
// parameters its spawner gave it.
static u16 child_key(struct Object *obj) {
    u32 key = (u32) obj->behavior ^ ((u32) obj->behavior >> 13) ^ obj->oBehParams;

    key ^= key >> 16;
    key ^= key >> 7;
    return key & CHILD_KEY_MASK;
}

// The name a spawned enemy has for the numbered object `spawner` behind it.
static u16 child_name(u32 spawner, struct Object *obj) {
    return CHILD_NAME_BIT | (spawner << CHILD_KEY_BITS) | child_key(obj);
}

// The numbered object behind the enemy `obj`, if one spawned it: its parent's number, or, on the
// `nested` pass, the number in the name its parent was given on the pass before, so an enemy
// that a spawned enemy spawned is named too. 0 if it has none.
static u32 spawner_behind(struct Object *obj, s32 nested) {
    s32 parentSlot;

    if (obj->parentObj == obj || obj->parentObj == NULL || obj->header.gfx.sharedChild == NULL
        || mp_object_id(obj) != 0) {
        return 0;
    }
    if (!nested) {
        return mp_object_id(obj->parentObj);
    }
    parentSlot = slot_of(obj->parentObj);
    if (parentSlot < 0 || parentSlot >= OBJECT_POOL_CAPACITY
        || mp_object_id(obj->parentObj) != 0) {
        return 0;
    }
    return owning_number(sCopies->childName[parentSlot]);
}

// Names, on one pass, every enemy with a model in the synced lists that has a spawner behind it,
// and lists its slot after the `count` found so far. Returns the new count.
static s32 name_children_pass(s32 nested, s32 count) {
    struct ObjectNode *list;
    struct ObjectNode *node;
    struct Object *obj;
    u32 spawner;
    s32 slot;
    s32 i;

    for (i = 0; i < NUM_OBJ_LISTS; i++) {
        if (!is_synced_list(i)) {
            continue;
        }
        list = &gObjectLists[i];
        for (node = list->next; node != list && count < OBJECT_POOL_CAPACITY; node = node->next) {
            obj = (struct Object *) node;
            slot = slot_of(obj);
            if (slot < 0 || slot >= OBJECT_POOL_CAPACITY || sCopies->childName[slot] != 0
                || (spawner = spawner_behind(obj, nested)) == 0) {
                continue;
            }
            sCopies->childName[slot] = child_name(spawner, obj);
            sCopies->foundSlots[count] = slot;
            count++;
        }
    }
    return count;
}

// Names every enemy with a model that a numbered object spawned, this frame, and every one that
// such an enemy spawned. Two that would share a name get none, so one is never taken for the
// other; unless one was made here for another game's spawner and this game's own spawner has now
// spawned the same enemy, in which case the one made here goes and the spawner's own has the name.
static void name_children(void) {
    u8 *slots = sCopies->foundSlots;
    u8 unnamed[OBJECT_POOL_CAPACITY / 8 + 1];
    s32 count;
    s32 a;
    s32 b;
    s32 i;
    s32 j;

    for (i = 0; i < OBJECT_POOL_CAPACITY; i++) {
        sCopies->childName[i] = 0;
    }
    for (i = 0; i < (s32) sizeof(unnamed); i++) {
        unnamed[i] = 0;
    }
    if (sIds->levelNum != gCurrLevelNum || sIds->areaIndex != gCurrAreaIndex) {
        return;
    }
    count = name_children_pass(FALSE, 0);
    count = name_children_pass(TRUE, count);
    for (i = 0; i < count; i++) {
        for (j = i + 1; j < count; j++) {
            if (sCopies->childName[slots[i]] != sCopies->childName[slots[j]]) {
                continue;
            }
            a = sCopies->slotFlags[slots[i]] & SLOT_NET_SPAWNED;
            b = sCopies->slotFlags[slots[j]] & SLOT_NET_SPAWNED;
            if (a == b || a) {
                unnamed[i / 8] |= 1 << (i % 8);
            }
            if (a == b || b) {
                unnamed[j / 8] |= 1 << (j % 8);
            }
            if (a != b) {
                obj_mark_for_deletion(&gObjectPool[slots[a ? i : j]]);
            }
        }
    }
    for (i = 0; i < count; i++) {
        if (unnamed[i / 8] & (1 << (i % 8))) {
            sCopies->childName[slots[i]] = 0;
        }
    }
    for (i = 0; i < OBJECT_POOL_CAPACITY; i++) {
        if (sCopies->childName[i] == 0) {
            // Whatever is in the slot now, it is not an enemy made here for another game.
            sCopies->slotFlags[i] &= ~SLOT_NET_SPAWNED;
            sCopies->childAge[i] = 0;
        } else if (sCopies->childAge[i] < 255) {
            sCopies->childAge[i]++;
        }
    }
}

// Takes the bridge's last consistent word on ownership into `kept`, and says whether what is
// kept is about the current area. A read the bridge was writing through leaves the word from
// before: for that one frame every copy would otherwise run on its own toward this game's
// Mario, nothing would be published, and every spawned enemy would count as this game's.
static s32 read_owned(struct MpObjectsOwned *kept) {
    volatile struct MpObjectsOwned *owned = &gMpMailbox->objectsOwned;
    struct MpObjectsOwned read;
    u32 sequence = owned->sequence;
    s32 i;

    if (gMpMailbox->bridgeLease == 0) {
        // No bridge: nothing kept from one is believed, even from one that was killed half
        // way through writing this, which left the record never to read as whole again.
        kept->levelNum = MP_NO_LEVEL;
        return FALSE;
    }
    if (!(sequence & 1)) {
        read.levelNum = owned->levelNum;
        read.areaIndex = owned->areaIndex;
        for (i = 0; i < MP_OBJECT_MASK_BYTES; i++) {
            read.owned[i] = owned->owned[i];
            read.gone[i] = owned->gone[i];
        }
        if (owned->sequence == sequence) {
            kept->levelNum = read.levelNum;
            kept->areaIndex = read.areaIndex;
            for (i = 0; i < MP_OBJECT_MASK_BYTES; i++) {
                kept->owned[i] = read.owned[i];
                kept->gone[i] = read.gone[i];
            }
        }
    }
    return kept->levelNum == gCurrLevelNum && kept->areaIndex == gCurrAreaIndex;
}

#define F32_EXPONENT_MASK 0x7F800000

s32 mp_is_finite(f32 value) {
    union {
        f32 f;
        u32 bits;
    } number;

    number.f = value;
    return (number.bits & F32_EXPONENT_MASK) != F32_EXPONENT_MASK;
}

static s32 is_ram_pointer(void *pointer) {
    return (u32) pointer >= RAM_START && (u32) pointer < RAM_END;
}

// Whether `index` names one of the object's own animations. Animation lists have no length, so
// every entry up to it must be a real pointer; the owner's game never sends anything else.
static s32 is_anim_index(struct Object *obj, s32 index) {
    struct Animation **anims = obj->oAnimations;
    s32 i;

    if (index < 0 || index >= MAX_ANIM_INDEX || !is_ram_pointer(anims)) {
        return FALSE;
    }
    for (i = 0; i <= index; i++) {
        if (!is_ram_pointer(anims[i])) {
            return FALSE;
        }
    }
    return TRUE;
}

// Where the object's current animation is in its own list, or -1.
static s8 anim_index_of(struct Object *obj) {
    struct Animation **anims = obj->oAnimations;
    s32 i;

    if (!is_ram_pointer(anims)) {
        return -1;
    }
    for (i = 0; i < MAX_ANIM_INDEX && is_ram_pointer(anims[i]); i++) {
        if (anims[i] == obj->header.gfx.animInfo.curAnim) {
            return i;
        }
    }
    return -1;
}

static void copy_state(struct MpObjectState *to, volatile struct MpObjectState *from) {
    volatile u32 *source = (volatile u32 *) from;
    u32 *destination = (u32 *) to;
    u32 i;

    for (i = 0; i < sizeof(struct MpObjectState) / sizeof(u32); i++) {
        destination[i] = source[i];
    }
}

// The object's record as a copy, if another game runs it and a state came lately, whether or
// not it is mirroring right now. It must also have run once, so its behavior script has set up
// its hitbox and model.
static struct MpCopy *copy_of(struct Object *obj) {
    s32 slot = slot_of(obj);
    struct MpCopy *copy;

    if (slot < 0 || slot >= OBJECT_POOL_CAPACITY) {
        return NULL;
    }
    copy = &sCopies->slots[slot];
    if (copy->id == 0 || copy->framesSinceState >= FOLLOW_FRAMES || !is_others(copy->id)
        || name_of(slot) != copy->id || obj->curBhvCommand == obj->behavior) {
        return NULL;
    }
    return copy;
}

// Whether this game's Mario holds `obj`. Asked of Mario, not of the object's oHeldState: that
// field is only what it says in objects that can be held, and others keep something else there,
// as a goomba does how fast it walks, which made every goomba look held.
static s32 is_held_here(struct Object *obj) {
    return gMarioState->heldObj == obj;
}

// Whether this game's Mario waits on `obj` to let him go: it has grabbed him, as a Chuckya does,
// or he has stopped to talk to it. Only the object's own behavior ends either, and neither can
// be hit out of, so an object that did not run here would hold him for good.
static s32 is_engaged_here(struct Object *obj) {
    return gMarioState->usedObj == obj
        && (gMarioState->action == ACT_GRABBED || gMarioState->action == ACT_WAITING_FOR_DIALOG
            || gMarioState->action == ACT_READING_NPC_DIALOG);
}

// Whether this object is a copy that should mirror its owner this frame: this game's Mario does
// not hold it, and this game has not claimed it. Nor has it stopped time here, as a character
// does while it speaks: only its own behavior starts time again, so whoever the server says
// runs it, it runs here until it has, or this game would stand still for good.
static struct MpCopy *following(struct Object *obj) {
    struct MpCopy *copy = copy_of(obj);

    if (copy == NULL || is_held_here(obj) || copy->claim != 0
        || ((obj->activeFlags & ACTIVE_FLAG_INITIATED_TIME_STOP)
            && (gTimeStopState & TIME_STOP_ENABLED))) {
        return NULL;
    }
    return copy;
}

static void follow_looks(struct Object *obj, struct MpCopy *copy) {
    struct MpObjectState *target = &copy->target;
    struct GraphNodeObject *gfx = &obj->header.gfx;

    obj->oOpacity = target->opacity;
    obj->oIntangibleTimer = (target->flags & MP_OBJECT_INTANGIBLE) ? -1 : 0;
    gfx->scale[0] = target->scale[0] / SCALE_ONE;
    gfx->scale[1] = target->scale[1] / SCALE_ONE;
    gfx->scale[2] = target->scale[2] / SCALE_ONE;
    gfx->node.flags &= ~(GRAPH_RENDER_ACTIVE | GRAPH_RENDER_INVISIBLE);
    if (target->flags & MP_OBJECT_RENDERED) {
        gfx->node.flags |= GRAPH_RENDER_ACTIVE;
    }
    if (target->flags & MP_OBJECT_INVISIBLE) {
        gfx->node.flags |= GRAPH_RENDER_INVISIBLE;
    }
    if (is_anim_index(obj, target->animIndex)) {
        // gCurrentObject is this object: mp_cur_obj_update runs in its place in the update loop.
        cur_obj_init_animation(target->animIndex);
        // The renderer moves the frame on between states; a new state brings it back in step.
        if (copy->framesSinceState == 0) {
            // A negative frame would index before the animation's data.
            gfx->animInfo.animFrame = target->animFrame < 0 ? 0 : target->animFrame;
        }
    }
    vec3f_copy(gfx->pos, &obj->oPosX);
    gfx->angle[0] = obj->oFaceAnglePitch;
    gfx->angle[1] = obj->oFaceAngleYaw;
    gfx->angle[2] = obj->oFaceAngleRoll;
}

// Puts a copy where the owner's is, and makes it look as the owner's does. The bridge gives each
// state as the object is to be drawn at that moment, between the states its owner sent, so there
// is nothing to smooth here: this game's own frame rate must not decide how a copy moves.
static void follow_motion(struct Object *obj, struct MpCopy *copy) {
    struct MpObjectState *target = &copy->target;

    obj->oPosX = target->pos[0];
    obj->oPosY = target->pos[1];
    obj->oPosZ = target->pos[2];
    obj->oVelY = target->velY;
    obj->oForwardVel = target->forwardVel;
    obj->oFaceAngleYaw = target->faceYaw;
    obj->oMoveAngleYaw = target->moveYaw;
    obj->oFaceAnglePitch = target->facePitch;
    obj->oFaceAngleRoll = target->faceRoll;
    follow_looks(obj, copy);
}

// One frame of a copy: it is where the owner's is, does what it does, and looks as it does.
static void follow_owner(struct Object *obj, struct MpCopy *copy) {
    struct MpObjectState *target = &copy->target;

    follow_motion(obj, copy);
    obj->oAction = target->action;
    // The game resets the timer and subaction when it sees the action change; these already match.
    obj->oPrevAction = target->action;
    obj->oSubAction = target->subAction;
    obj->oTimer = target->timer + copy->framesSinceState;
    obj->oHealth = target->health;
}

// The owner's latest state for the spawner `obj`, if this game's is a copy that does as the
// owner's does without running: see MP_OBJECT_REPLICATES. Like any copy, it has run once.
static struct MpCopy *mirrored_spawner(struct Object *obj) {
    s32 slot = slot_of(obj);
    struct MpCopy *copy;
    u16 id;

    if (slot < 0 || slot >= OBJECT_POOL_CAPACITY || sIds->slots[slot].synced != SYNC_REPORTED) {
        return NULL;
    }
    copy = &sCopies->slots[slot];
    id = mp_object_id(obj);
    if (id == 0 || copy->id != id || copy->framesSinceState >= FOLLOW_FRAMES || !is_others(id)
        || !(copy->target.flags & MP_OBJECT_REPLICATES) || obj->curBhvCommand == obj->behavior) {
        return NULL;
    }
    return copy;
}

static f32 distance_squared(struct Object *a, struct Object *b) {
    f32 dx = a->oPosX - b->oPosX;
    f32 dy = a->oPosY - b->oPosY;
    f32 dz = a->oPosZ - b->oPosZ;

    return dx * dx + dy * dy + dz * dz;
}

// The player a shared object this game runs is to see as Mario: another player who is clearly
// nearer it than the local one, or the local one. An object is run by the game of the player
// nearest it, give or take, or of the one who last attacked it, and would otherwise chase, flee
// and wake for that game's Mario alone, whoever else stood beside it.
static struct Object *player_seen_by(struct Object *obj) {
    struct Object *seen = gMarioObject;
    struct Object *puppet;
    f32 nearest;
    f32 distance;
    s32 slot = slot_of(obj);
    s32 i;

    // What holds this game's Mario, or is held by him, deals with him: what it does to the
    // Mario it sees, as throwing him, would be lost on a puppet.
    if (!sCopies->knowsOwners || seen == NULL || slot < 0 || slot >= OBJECT_POOL_CAPACITY
        || is_held_here(obj) || is_engaged_here(obj)
        || (sCopies->childName[slot] == 0
            && (mp_object_id(obj) == 0 || sIds->slots[slot].synced == SYNC_NONE))) {
        return seen;
    }
    nearest = distance_squared(obj, seen) / NEARER_BY;
    for (i = 0; i < MP_PUPPETS; i++) {
        puppet = mp_visible_puppet(i);
        if (puppet != NULL && (distance = distance_squared(obj, puppet)) < nearest) {
            nearest = distance;
            seen = puppet;
        }
    }
    return seen;
}

// Runs the current object's own behavior. For one it shares with other games, Mario's object is
// for that long the nearest player's: a puppet has a place, a facing and a size as Mario's has,
// which is what a behavior reads there, and what it works out from them, such as how far Mario
// is and which way. What a behavior reads of Mario's state, as whether he is in the air, is
// still the local Mario's, and what it does to Mario it does through his own interactions, in
// his own game.
static void run_behavior(struct Object *obj) {
    struct Object *local = gMarioObject;
    struct Object *seen = player_seen_by(obj);

    gMarioObject = seen;
    cur_obj_update();
    gMarioObject = local;
    if (seen != local) {
        // What it marked there was for a Mario's interactions, which a puppet has none of.
        seen->oInteractStatus = 0;
    }
}

// Every object's update goes through here instead of cur_obj_update; see Multiplayer/hooks.asm.
void mp_cur_obj_update(void) {
    struct Object *obj = gCurrentObject;
    struct MpCopy *copy = following(obj);

    if (copy == NULL && (copy = mirrored_spawner(obj)) != NULL) {
        // What its enemies ask of it, such as whether it has them loaded, is as in its owner's
        // game.
        obj->oAction = copy->target.action;
    } else if (copy == NULL) {
        run_behavior(obj);
    } else if (copy->target.flags & MP_OBJECT_PARENT) {
        // Its behavior runs, so it spawns its own game's enemies, and then it is put where the
        // owner's is.
        cur_obj_update();
        follow_motion(obj, copy);
    } else {
        follow_owner(obj, copy);
    }
}

static s32 is_valid_state(volatile struct MpObjectState *state) {
    return mp_is_finite(state->pos[0]) && mp_is_finite(state->pos[1])
        && mp_is_finite(state->pos[2])
        && mp_is_finite(state->velY) && mp_is_finite(state->forwardVel);
}

// Makes the spawned enemy a state is about, which another game's spawner spawned and this game's
// has not, if it is of a kind games make for themselves. Returns its slot, or NO_SLOT.
//
// Everything is checked against this game: the kind picks from its own list, the spawner must be
// the numbered object the name is for, and the enemy made must come out with the state's name,
// which is of its behavior and parameters, or it is removed again.
static u8 spawn_copy(volatile struct MpObjectState *state) {
    u32 name = state->id;
    s32 kind = state->flags >> MP_OBJECT_KIND_SHIFT;
    struct Object *spawner = mp_numbered_object(owning_number(name));
    const struct MpSpawnKind *made;
    struct Object *obj;
    s32 slot;

    if (!is_child_name(name) || kind < 1 || kind > SPAWN_KINDS || spawner == NULL) {
        return NO_SLOT;
    }
    made = &sSpawnKinds[kind - 1];
    // A level loads only the models its own enemies use.
    if (gLoadedGraphNodes[made->model] == NULL) {
        return NO_SLOT;
    }
    obj = try_to_spawn_object(0, 1.0f, spawner, made->model, made->behavior);
    if (obj == NULL) {
        return NO_SLOT;
    }
    obj->oBehParams = state->behParams;
    obj->oBehParams2ndByte = (state->behParams >> 16) & 0xFF;
    // A goomba, the one kind made here, looks its size up by these bits in a list of three:
    // the fourth is none, and what follows the list would be its scale and its damage.
    if ((obj->oBehParams2ndByte & GOOMBA_BP_SIZE_MASK) > GOOMBA_SIZE_TINY
        || child_name(owning_number(name), obj) != name) {
        obj_mark_for_deletion(obj);
        return NO_SLOT;
    }
    slot = slot_of(obj);
    sCopies->childName[slot] = name;
    sCopies->slotFlags[slot] = SLOT_NET_SPAWNED;
    sCopies->childAge[slot] = 0;
    return slot;
}

// A spawner keeps what it remembers in its parameters, as a goomba triplet's spawner does which
// of its goombas are dead; only the game that runs it sees them die. Its copies take those bits,
// so that one does not spawn an enemy again that was defeated in another game.
static void take_spawner_state(volatile struct MpObjectState *state) {
    struct Object *obj = synced_object(state->id);
    struct MpCopy *copy;

    if (obj != NULL && state->check == behavior_check(obj)) {
        obj->oBehParams |= state->behParams;
        // Kept for mirrored_spawner, which says whether this copy is to run.
        copy = &sCopies->slots[slot_of(obj)];
        copy_state(&copy->target, state);
        copy->id = state->id;
        copy->framesSinceState = 0;
    }
}

// Takes the states the bridge added since the last frame, for copies to mirror.
static void take_object_states(void) {
    volatile struct MpObjectsIn *in = &gMpMailbox->objectsIn;
    struct MpObjectState taken;
    struct MpObjectState *state = &taken;
    struct MpCopy *copy;
    u32 count = in->count;
    u32 behind = count - sIds->objectsInTaken;
    u8 slot;

    if (behind > MP_OBJECTS_IN_ENTRIES) {
        // Older entries were overwritten; or the count went backwards, so start again from it.
        sIds->objectsInTaken = behind < 0x80000000 ? count - MP_OBJECTS_IN_ENTRIES : count;
    }
    while (sIds->objectsInTaken != count) {
        // Taken out of the mailbox once, and only the copy looked at: the bridge may write
        // the entry again meanwhile, and what was checked must be what is used.
        copy_state(state, &in->entries[sIds->objectsInTaken % MP_OBJECTS_IN_ENTRIES]);
        sIds->objectsInTaken++;
        if (state->levelNum != gCurrLevelNum || state->areaIndex != gCurrAreaIndex
            || !is_others(state->id) || !is_valid_state(state)) {
            continue;
        }
        slot = named_slot(state->id);
        if (slot == NO_SLOT && !is_child_name(state->id)) {
            // Only mirrored objects and spawned enemies have names: this is a spawner's.
            take_spawner_state(state);
            continue;
        }
        if (slot == NO_SLOT && (slot = spawn_copy(state)) == NO_SLOT) {
            continue;
        }
        copy = &sCopies->slots[slot];
        if (copy->claim != 0 || state->check != behavior_check(&gObjectPool[slot])) {
            continue;
        }
        copy_state(&copy->target, state);
        copy->id = state->id;
        copy->framesSinceState = 0;
    }
    // The bridge gives no more than the ring holds beyond what has been taken.
    gMpMailbox->objectsInTaken = sIds->objectsInTaken;
}

// The bridge's last consistent copy of the server's list of spawned enemies alive in this area.
static void read_live_children(void) {
    volatile struct MpLiveChildren *live = &gMpMailbox->liveChildren;
    u32 sequence = live->sequence;
    u32 count = live->count;
    u32 i;

    sCopies->liveKnown = FALSE;
    if ((sequence & 1) || count > MP_LIVE_CHILDREN || live->levelNum != gCurrLevelNum
        || live->areaIndex != gCurrAreaIndex) {
        return;
    }
    for (i = 0; i < count; i++) {
        sCopies->liveNames[i] = live->names[i];
    }
    sCopies->liveCount = count;
    sCopies->liveKnown = live->sequence == sequence;
}

static s32 is_live_child(u32 name) {
    s32 i;

    for (i = 0; i < sCopies->liveCount; i++) {
        if (sCopies->liveNames[i] == name) {
            return TRUE;
        }
    }
    return FALSE;
}

// Removes the spawned enemies another game runs that are no longer alive there: defeated, or
// unloaded with their spawner. That a game spawned one for itself does not keep it: whoever runs
// the spawner says which of its enemies there are. One just named, or heard of, is left a while.
static void remove_dead_children(void) {
    struct MpCopy *copy;
    u16 name;
    s32 slot;

    read_live_children();
    if (!sCopies->liveKnown) {
        return;
    }
    for (slot = 0; slot < OBJECT_POOL_CAPACITY; slot++) {
        name = sCopies->childName[slot];
        copy = &sCopies->slots[slot];
        if (name == 0 || !is_others(name) || copy->claim != 0
            || sCopies->childAge[slot] <= CHILD_GRACE_FRAMES
            || (copy->id == name && copy->framesSinceState <= STATE_GRACE_FRAMES)
            || is_live_child(name)) {
            continue;
        }
        drop_loot(&gObjectPool[slot], name);
        obj_mark_for_deletion(&gObjectPool[slot]);
    }
}

// Has a copy that is about to go drop the coins its owner's dropped, if that one was defeated:
// when last heard of it was squished or knocked back, as an enemy that Mario's attack has
// defeated is, and not walking about, as one unloaded with its spawner is. A copy's behavior
// does not run, so it would not drop them, and only the player whose game ran the enemy would
// see its coins. Enemies that die another way still drop theirs only in that game.
static void drop_loot(struct Object *obj, u32 name) {
    struct MpCopy *copy = &sCopies->slots[slot_of(obj)];

    if (copy->id == name && copy->framesSinceState < FOLLOW_FRAMES
        && copy->target.action >= OBJ_ACT_HORIZONTAL_KNOCKBACK
        && copy->target.action <= OBJ_ACT_SQUISHED && obj->oNumLootCoins > 0) {
        obj_spawn_loot_yellow_coins(obj, obj->oNumLootCoins, 20.0f);
    }
}

// Removes the objects that their owners report gone, such as defeated enemies. An area loaded
// again, as when Mario dies in it, has them all back, and this game is given those nearest it
// to run: the server takes an object's going as final while anyone is in the area, so on the
// first word after a load they go again whoever runs them, as they have for everyone else.
static void remove_gone_objects(void) {
    struct Object *obj;
    s32 all = sIds->goneUnchecked;
    u32 id;

    for (id = 1; id <= sIds->count; id++) {
        if (mask_has(sCopies->owned.gone, id) && (all || is_others(id))
            && (obj = named_object(id)) != NULL) {
            drop_loot(obj, id);
            obj_mark_for_deletion(obj);
        }
    }
    sIds->goneUnchecked = FALSE;
}

void mp_take_object_states(void) {
    s32 i;

    // Without a word from the server, as with no bridge, every object runs as it would alone.
    sCopies->knowsOwners = read_owned(&sCopies->owned);
    note_loot();
    note_slots_left();
    name_children();
    for (i = 0; i < OBJECT_POOL_CAPACITY; i++) {
        if (sCopies->slots[i].framesSinceState < FOLLOW_FRAMES) {
            sCopies->slots[i].framesSinceState++;
        }
        // A claim is on an object, not on its slot: once the object has gone, whatever takes
        // the slot next, the first to be free, is another game's to run again.
        if (sCopies->slots[i].claim != 0 && name_of(i) != sCopies->slots[i].id) {
            sCopies->slots[i].claim = 0;
        }
        // Nor does it outlast its purpose: once the server has made this game the owner, the
        // claim is over. Kept to its end, it would have this game run the object on after
        // the server, which holds a claim a shorter time, gave it to a nearer player's.
        if (sCopies->slots[i].claim != 0 && sCopies->knowsOwners
            && !is_others(sCopies->slots[i].id)) {
            sCopies->slots[i].claim = 0;
        }
        if (sCopies->slots[i].claim != 0) {
            sCopies->slots[i].claim--;
        }
        // What this game runs has no state to follow. One kept from before it ran the
        // object would be followed again, seconds old, in the frame the object passed back
        // to another game: the enemy would flick to where it was then.
        if (sCopies->slots[i].id != 0 && sCopies->knowsOwners
            && (!is_others(sCopies->slots[i].id) || sCopies->slots[i].claim != 0)) {
            sCopies->slots[i].framesSinceState = FOLLOW_FRAMES;
        }
    }
    take_object_states();
    take_coins();
    if (sCopies->knowsOwners) {
        remove_gone_objects();
        remove_dead_children();
    }
}

static void post_interaction(u16 id, u32 status) {
    volatile struct MpInteractionRing *out = &gMpMailbox->interactionsOut;
    volatile struct MpInteraction *entry;
    u32 count = out->count;

    entry = &out->entries[count % MP_INTERACTION_ENTRIES];
    entry->levelNum = gCurrLevelNum;
    entry->areaIndex = gCurrAreaIndex;
    entry->id = id;
    entry->status = status;
    // Last, so the bridge never reads a half-written entry.
    out->count = count + 1;
}

// Coins are shared: one any player collects goes, and counts, in every game in the level.

// The most a coin is worth, a blue coin's 5 with room to spare; anything more is not a coin.
#define MAX_COIN_VALUE 10
#define MAX_COINS 999

static s32 is_coin(struct Object *obj) {
    return obj->activeFlags != ACTIVE_FLAG_DEACTIVATED && (obj->oInteractType & INTERACT_COIN);
}

// A coin's name: its number if the area placed it, or, if a numbered object spawned it, as a coin
// ring spawns its coins, or a defeated enemy dropped it, a name like a spawned enemy's. 0 for any
// other coin.
static u16 coin_name(struct Object *obj) {
    u16 id = number_at_load(obj);
    u16 parent;

    if (id != 0) {
        return id;
    }
    if (sIds->lootSpawner[slot_of(obj)] != 0) {
        return child_name(sIds->lootSpawner[slot_of(obj)], obj);
    }
    if (obj->parentObj == obj || obj->parentObj == NULL
        || (parent = mp_object_id(obj->parentObj)) == 0) {
        return 0;
    }
    return CHILD_NAME_BIT | (parent << CHILD_KEY_BITS) | child_key(obj);
}

static void post_coin(u16 name, s32 value) {
    volatile struct MpCoinRing *out = &gMpMailbox->coinsOut;
    volatile struct MpCoin *entry;
    u32 count = out->count;

    entry = &out->entries[count % MP_COIN_ENTRIES];
    entry->levelNum = gCurrLevelNum;
    entry->areaIndex = gCurrAreaIndex;
    entry->value = value;
    entry->name = name;
    // Last, so the bridge never reads a half-written entry.
    out->count = count + 1;
}

// Reports each coin Mario collected this frame. His interaction marks it, and the coin's own
// behavior removes it later in the frame.
void mp_report_coins(void) {
    struct Object *obj;
    s32 slot;

    for (slot = 0; slot < OBJECT_POOL_CAPACITY; slot++) {
        obj = &gObjectPool[slot];
        if (!is_coin(obj) || !(obj->oInteractStatus & INT_STATUS_INTERACTED)) {
            sCopies->slotFlags[slot] &= ~SLOT_COIN_TOLD;
            continue;
        }
        if (!(sCopies->slotFlags[slot] & SLOT_COIN_TOLD)) {
            sCopies->slotFlags[slot] |= SLOT_COIN_TOLD;
            if (obj->oDamageOrCoinValue > 0 && obj->oDamageOrCoinValue <= MAX_COIN_VALUE) {
                post_coin(coin_name(obj), obj->oDamageOrCoinValue);
            }
        }
    }
}

// The coin named `name` in this area that nobody has collected here yet, if there is one.
static struct Object *uncollected_coin(u16 name) {
    struct Object *obj;
    s32 slot;

    for (slot = 0; slot < OBJECT_POOL_CAPACITY; slot++) {
        obj = &gObjectPool[slot];
        if (is_coin(obj) && !(obj->oInteractStatus & INT_STATUS_INTERACTED)
            && coin_name(obj) == name) {
            return obj;
        }
    }
    return NULL;
}

// Collects `coin` here as another player collected it there, so its own behavior removes it
// with its sparkle, and a red coin still counts toward its star.
static void take_coin(struct Object *coin) {
    // Taken here, not collected by this game's Mario, so it is not reported back.
    sCopies->slotFlags[slot_of(coin)] |= SLOT_COIN_TOLD;
    coin->oInteractStatus |= INT_STATUS_INTERACTED;
}

// Counts the coins other players in this level collected, and collects the same coin here; and
// then any owed from before that have appeared since.
static void take_coins(void) {
    volatile struct MpCoinRing *in = &gMpMailbox->coinsIn;
    volatile struct MpCoin *entry;
    struct Object *coin;
    u32 count = in->count;
    u32 behind = count - sCopies->coinsInTaken;
    u32 value;
    s32 i;

    if (behind > MP_COIN_ENTRIES) {
        sCopies->coinsInTaken = behind < 0x80000000 ? count - MP_COIN_ENTRIES : count;
    }
    while (sCopies->coinsInTaken != count) {
        entry = &in->entries[sCopies->coinsInTaken % MP_COIN_ENTRIES];
        sCopies->coinsInTaken++;
        value = entry->value;
        if (entry->levelNum != gCurrLevelNum || value > MAX_COIN_VALUE) {
            continue;
        }
        // Worth nothing: one collected before this game came to the area, which only goes.
        gMarioState->numCoins += value;
        if (gMarioState->numCoins > MAX_COINS) {
            gMarioState->numCoins = MAX_COINS;
        }
        if (entry->areaIndex != gCurrAreaIndex || entry->name == 0) {
            continue;
        }
        if ((coin = uncollected_coin(entry->name)) != NULL) {
            take_coin(coin);
        } else if (value == 0) {
            // One collected before this game came, of a ring or a line that has no coins
            // here yet: owed until it has. Only of those: a coin another player collects now
            // and that is not here was this game's Mario's too, and is gone.
            for (i = 0; i < OWED_COINS && sIds->owedCoins[i] != 0; i++) {
            }
            if (i < OWED_COINS) {
                sIds->owedCoins[i] = entry->name;
            }
        }
    }
    // The bridge holds back what would not fit the ring until these have been taken.
    gMpMailbox->coinsInTaken = sCopies->coinsInTaken;
    for (i = 0; i < OWED_COINS; i++) {
        if (sIds->owedCoins[i] != 0 && (coin = uncollected_coin(sIds->owedCoins[i])) != NULL) {
            take_coin(coin);
            sIds->owedCoins[i] = 0;
        }
    }
}

void mp_claim_touched_objects(void) {
    struct MpCopy *copy;
    struct Object *obj;
    s32 slot;

    for (slot = 0; slot < OBJECT_POOL_CAPACITY; slot++) {
        obj = &gObjectPool[slot];
        if (sCopies->slots[slot].claim != 0 || (copy = copy_of(obj)) == NULL) {
            continue;
        }
        if ((obj->oInteractStatus & INT_STATUS_WAS_ATTACKED) || is_held_here(obj)
            || is_engaged_here(obj)) {
            // The status stays: the object's own behavior, which runs later this frame now
            // that it is claimed, reads it as it would in a game alone.
            post_interaction(copy->id, obj->oInteractStatus & FORWARDED_STATUS);
            copy->claim = CLAIM_FRAMES;
            sCopies->claimsMade++;
        } else {
            // Any other touch changes nothing the owner needs to hear of. A copy's behavior
            // does not run to clear the status, so it is cleared here.
            obj->oInteractStatus = 0;
        }
    }
}

static u16 saturate_u16(s32 value) {
    if (value < 0) {
        return 0;
    }
    return value > 0xFFFF ? 0xFFFF : value;
}

s16 mp_saturate_s16(f32 value) {
    if (value < -0x8000) {
        return -0x8000;
    }
    return value > 0x7FFF ? 0x7FFF : value;
}

static s8 saturate_s8(s32 value) {
    if (value < -0x80) {
        return -0x80;
    }
    return value > 0x7F ? 0x7F : value;
}

static u8 looks_flags(struct Object *obj) {
    u8 flags = 0;

    if (obj->oIntangibleTimer != 0) {
        flags |= MP_OBJECT_INTANGIBLE;
    }
    if (obj->header.gfx.node.flags & GRAPH_RENDER_ACTIVE) {
        flags |= MP_OBJECT_RENDERED;
    }
    if (obj->header.gfx.node.flags & GRAPH_RENDER_INVISIBLE) {
        flags |= MP_OBJECT_INVISIBLE;
    }
    return flags;
}

// Marks in `parents` each synced object that has spawned an enemy still about: an object in a
// synced list whose parent is a numbered object other than itself. And in `others`, each that has
// one of a kind games do not make for themselves.
static void find_parents(u8 *parents, u8 *others) {
    struct ObjectNode *list;
    struct ObjectNode *node;
    struct Object *obj;
    u16 id;
    s32 i;

    for (i = 0; i < MP_OBJECT_MASK_BYTES; i++) {
        parents[i] = 0;
        others[i] = 0;
    }
    for (i = 0; i < NUM_OBJ_LISTS; i++) {
        if (!is_synced_list(i)) {
            continue;
        }
        list = &gObjectLists[i];
        for (node = list->next; node != list; node = node->next) {
            obj = (struct Object *) node;
            if (obj->parentObj != obj && obj->parentObj != NULL
                && (id = mp_object_id(obj->parentObj)) != 0) {
                parents[id / 8] |= 1 << (id % 8);
                if (spawn_kind(obj) == 0) {
                    others[id / 8] |= 1 << (id % 8);
                }
            }
        }
    }
}

// `more` is what to add to the flags that say how the object looks: for a numbered object, that
// it has spawned enemies and whether other games make them all; for a spawned enemy, its kind.
static void publish_state(volatile struct MpObjectState *state, struct Object *obj, u16 id,
                          u32 more) {
    struct GraphNodeObject *gfx = &obj->header.gfx;

    state->levelNum = gCurrLevelNum;
    state->areaIndex = gCurrAreaIndex;
    state->pad = 0;
    state->id = id;
    state->action = saturate_u16(obj->oAction);
    state->pos[0] = obj->oPosX;
    state->pos[1] = obj->oPosY;
    state->pos[2] = obj->oPosZ;
    state->velY = obj->oVelY;
    state->forwardVel = obj->oForwardVel;
    state->faceYaw = obj->oFaceAngleYaw;
    state->moveYaw = obj->oMoveAngleYaw;
    state->facePitch = obj->oFaceAnglePitch;
    state->faceRoll = obj->oFaceAngleRoll;
    state->timer = saturate_u16(obj->oTimer);
    state->subAction = obj->oSubAction;
    state->health = saturate_s8(obj->oHealth);
    state->scale[0] = mp_saturate_s16(gfx->scale[0] * SCALE_ONE);
    state->scale[1] = mp_saturate_s16(gfx->scale[1] * SCALE_ONE);
    state->scale[2] = mp_saturate_s16(gfx->scale[2] * SCALE_ONE);
    state->animIndex = anim_index_of(obj);
    state->opacity = obj->oOpacity;
    state->animFrame = gfx->animInfo.animFrame;
    state->flags = looks_flags(obj) | more;
    state->check = behavior_check(obj);
    state->behParams = obj->oBehParams;
}

static void post_child_gone(u16 name) {
    volatile struct MpNameRing *out = &gMpMailbox->childrenGoneOut;
    u32 count = out->count;

    out->names[count % MP_NAME_ENTRIES] = name;
    // Last, so the bridge never reads a half-written entry.
    out->count = count + 1;
}

// Reports each spawned enemy this game ran last frame that is gone now, killed or unloaded, so the
// other games remove theirs. One that passed to another game is not gone.
static void report_children_gone(void) {
    u16 last;
    s32 slot;

    for (slot = 0; slot < OBJECT_POOL_CAPACITY; slot++) {
        last = sCopies->ownedChildLast[slot];
        if (last != 0 && sCopies->childName[slot] != last && named_slot(last) == NO_SLOT) {
            post_child_gone(last);
        }
        last = sCopies->childName[slot];
        sCopies->ownedChildLast[slot] = last != 0 && !is_others(last) ? last : 0;
    }
}

void mp_publish_objects(void) {
    volatile struct MpObjectsOut *out = &gMpMailbox->objectsOut;
    u8 parents[MP_OBJECT_MASK_BYTES];
    u8 others[MP_OBJECT_MASK_BYTES];
    struct Object *obj;
    u16 count = 0;
    u16 name;
    u32 more;
    u32 id;
    s32 slot;
    s32 i;

    find_parents(parents, others);
    out->sequence = out->sequence | 1;
    out->levelNum = gCurrLevelNum;
    out->areaIndex = gCurrAreaIndex;
    for (i = 0; i < MP_OBJECT_MASK_BYTES; i++) {
        out->gone[i] = 0;
    }
    // Numbers from an earlier area name nothing here, and synced_slot finds none. A spawner going
    // is not reported: each game's runs on its own.
    for (id = 1; id <= sIds->count; id++) {
        if ((slot = synced_slot(id)) == NO_SLOT) {
            continue;
        }
        obj = synced_object(id);
        if (obj == NULL) {
            if (sIds->slots[slot].synced == SYNC_MIRRORED) {
                out->gone[id / 8] |= 1 << (id % 8);
            }
        } else if (sCopies->knowsOwners && runs_here(id, slot)) {
            more = 0;
            if (mask_has(parents, id)) {
                more = mask_has(others, id) ? MP_OBJECT_PARENT
                                            : MP_OBJECT_PARENT | MP_OBJECT_REPLICATES;
            }
            publish_state(&out->states[count], obj, id, more);
            count++;
        }
    }
    // The pool holds at most OBJECT_POOL_CAPACITY objects, so numbered and spawned ones together
    // fit the out block.
    for (slot = 0; slot < OBJECT_POOL_CAPACITY && count < MP_OBJECT_IDS; slot++) {
        name = sCopies->childName[slot];
        if (name != 0 && sCopies->knowsOwners && runs_here(name, slot)) {
            // Its kind, so a game whose own spawner has not spawned this enemy can make it.
            more = spawn_kind(&gObjectPool[slot]) << MP_OBJECT_KIND_SHIFT;
            publish_state(&out->states[count], &gObjectPool[slot], name, more);
            count++;
        }
    }
    out->count = count;
    out->sequence = out->sequence + 1;
    report_children_gone();
}
