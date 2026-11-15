#include "sm64.h"
#include "area.h"
#include "engine/graph_node.h"
#include "engine/math_util.h"
#include "game/memory.h"
#include "level_update.h"
#include "object_fields.h"
#include "object_helpers.h"
#include "object_list_processor.h"
#include "rendering_graph_node.h"
#include "sounds.h"

#include "Multiplayer/mailbox.h"
#include "Multiplayer/objects.h"
#include "Multiplayer/progress.h"

// Animations 0xD1-0xD5, which Mario/anims.asm adds after the ROM's own table.
#define EXTRA_MARIO_ANIMS 5

#define ANIM_CACHE_ENTRIES 32
#define ANIM_CACHE_SIZE (MP_ANIM_CACHE_END - MP_ANIM_CACHE_ADDR)
// The PI DMA needs 8-byte aligned RAM; 16 keeps every animation on a cache line.
#define ANIM_ALIGNMENT 16

struct PuppetRuntime {
    struct Object *object;
    struct MpPlayerSlot slot; // Last consistent copy of this puppet's mailbox slot.
};

struct AnimCacheEntry {
    s16 animID;
    u8 inUse; // Whether a puppet has played it since the frame began.
    // One more than the animation that a puppet drawn with this one is waiting for, or 0.
    u8 wanted;
    struct Animation *anim;
};

// Mario's animations load into one shared buffer, so puppets keep their own copies here, one per
// animation however many puppets play it, in the order they were loaded. When one does not fit,
// those no puppet played in the frame are dropped at the start of the next, before any puppet
// runs, and the rest are moved together to make room. A puppet whose new animation has no room
// yet is drawn with its last one meanwhile, which stays for as long as dropping it would still
// not make the room. At first the whole cache was emptied instead: fifteen puppets changing among
// 48 animations then emptied it four times a second, and one of them was hidden in 4.5% of
// frames.
struct AnimCache {
    u32 used;
    s32 count;
    s32 compactPending;
    struct DmaHandlerList list;
    struct AnimCacheEntry entries[ANIM_CACHE_ENTRIES];
};

struct MpPrivate {
    struct PuppetRuntime puppets[MP_PUPPETS];
    struct AnimCache animCache;
    // How many entries of the mailbox's `hitsIn` ring have been taken.
    u32 hitsInTaken;
    // Whether the current swing has already hit each puppet, so one swing lands once per player.
    u8 swingLanded[MP_PUPPETS];
    // The camera's field of view, in degrees, as of the last Mario drawn; Multiplayer/people.c
    // places names with it.
    f32 fov;
};

// In RAM the survey found zeroed, so this needs no initialization at boot.
#define sPrivate ((struct MpPrivate *) MP_PRIVATE_ADDR)
// World.c keeps its stash right after this.
typedef char MpPrivateSizeCheck[sizeof(struct MpPrivate) <= MP_WORLD_STASH_ADDR - MP_PRIVATE_ADDR
                                    ? 1
                                    : -1];

// How far past touching a punch, kick or trip reaches.
#define HIT_REACH 60.0f
// Half the arc in front of Mario that his punches and kicks reach, as for the game's own enemies.
#define STRIKE_ARC 0x2AAA
// Trips sweep a wider arc, as in the game's own interactions.
#define TRIP_ARC 0x4000

// Health wedges any hit takes; the game adds half again when Mario has no cap. Kinds could differ.
#define HIT_DAMAGE 1
// A puppet fades in when it appears and out when it goes, by this much opacity a frame: about
// half a second at the game's usual rate.
#define FADE_STEP 17
#define OPAQUE 255
// In Mario's model state, what says he is see-through, as under the vanish cap; the low byte is
// how opaque he still is.
#define MODEL_STATE_ALPHA 0x100
// Tools that test a running game read puppet object pointers at this stride.
typedef char PuppetRuntimeSizeCheck[sizeof(struct PuppetRuntime) == 0x84 ? 1 : -1];

extern const BehaviorScript bhvMpPuppet[];
extern void bhv_mario_update(void);
// The game's own, which Mario's model calls as it is drawn; trollfinity.asm lacks them.
extern Gfx *geo_mirror_mario_set_alpha(s32 callContext, struct GraphNode *node, Mat4 *mtx);
extern Gfx *geo_switch_mario_cap_effect(s32 callContext, struct GraphNode *node, Mat4 *mtx);
extern s32 mario_anim_load_patchable_table(struct DmaHandlerList *list, s32 index);
extern u32 mario_patchable_table_TWO[];
extern struct Object *try_to_spawn_object(s16 offsetY, f32 scale, struct Object *parent, s32 model,
                                          const BehaviorScript *behavior);
extern void push_mario_out_of_object(struct MarioState *m, struct Object *o, f32 padding);
// The steps of the game's own take_damage_and_knock_back, which Troll Engine does not map.
extern s32 take_damage_from_interact_object(struct MarioState *m);
extern u32 determine_knockback_action(struct MarioState *m, s32 damage);
extern s32 drop_and_set_mario_action(struct MarioState *m, u32 action, u32 actionArg);
extern void play_sound(s32 soundBits, f32 *pos);

static s32 anim_count(void) {
    return (s32) gMarioState->animList->dmaTable->count + EXTRA_MARIO_ANIMS;
}

static s32 is_slot_valid(struct MpPlayerSlot *slot) {
    return mp_is_finite(slot->pos[0]) && mp_is_finite(slot->pos[1])
        && mp_is_finite(slot->pos[2]) && slot->animID >= 0 && slot->animID < anim_count();
}

// `animID` must already be validated.
static u32 anim_size(s32 animID) {
    struct DmaTable *table = gMarioState->animList->dmaTable;

    if ((u32) animID < table->count) {
        return table->anim[animID].size;
    }
    return mario_patchable_table_TWO[(animID - table->count) * 2 + 1];
}

static u32 anim_cache_align(u32 offset) {
    return (offset + ANIM_ALIGNMENT - 1) & ~(ANIM_ALIGNMENT - 1);
}

// Moves a cached animation down to `to`, which may overlap where it is, and returns it there.
static struct Animation *anim_cache_move(struct Animation *from, struct Animation *to, u32 size) {
    u32 *source = (u32 *) from;
    u32 *destination = (u32 *) to;
    u32 moved = (u32) from - (u32) to;
    u32 i;

    // Lowest word first, so an overlapping move never reads a word it has written.
    for (i = 0; i < (size + 3) / 4; i++) {
        destination[i] = source[i];
    }
    // Its tables are in the animation itself, so their pointers move with it.
    to->values = (void *) ((u32) to->values - moved);
    to->index = (void *) ((u32) to->index - moved);
    return to;
}

// Drops the animations no puppet played in the frame just ended and moves the rest together.
// Only at the start of a frame: every puppet then takes its animation's new place before it is
// drawn.
static void anim_cache_compact(struct AnimCache *cache) {
    struct AnimCacheEntry *entry;
    struct Animation *to;
    u32 used = 0;
    u32 start;
    s32 kept = 0;
    s32 i;

    for (i = 0; i < cache->count; i++) {
        entry = &cache->entries[i];
        if (!entry->inUse) {
            continue;
        }
        start = anim_cache_align(used);
        to = (struct Animation *) (MP_ANIM_CACHE_ADDR + start);
        if (to != entry->anim) {
            entry->anim = anim_cache_move(entry->anim, to, anim_size(entry->animID));
        }
        used = start + anim_size(entry->animID);
        cache->entries[kept] = *entry;
        kept++;
    }
    cache->used = used;
    cache->count = kept;
    cache->compactPending = FALSE;
}

// Keeps each animation a puppet is drawn with only for want of another, if dropping it would
// still leave no room for the one wanted: the puppet then goes on being drawn with it.
static void anim_cache_keep_held(struct AnimCache *cache) {
    struct AnimCacheEntry *entry;
    u32 kept = 0;
    u32 wanted;
    s32 i;

    for (i = 0; i < cache->count; i++) {
        if (cache->entries[i].inUse) {
            kept += anim_cache_align(anim_size(cache->entries[i].animID));
        }
    }
    for (i = 0; i < cache->count; i++) {
        entry = &cache->entries[i];
        if (entry->inUse || entry->wanted == 0) {
            continue;
        }
        wanted = anim_cache_align(anim_size(entry->wanted - 1));
        if (kept + wanted <= ANIM_CACHE_SIZE) {
            kept += wanted;
        } else {
            entry->inUse = TRUE;
            kept += anim_cache_align(anim_size(entry->animID));
        }
    }
}

// Starts a frame: makes room if an animation did not fit in the last one, and forgets which
// animations that frame's puppets played.
static void anim_cache_begin_frame(void) {
    struct AnimCache *cache = &sPrivate->animCache;
    s32 i;

    if (cache->compactPending) {
        anim_cache_keep_held(cache);
        anim_cache_compact(cache);
    }
    for (i = 0; i < cache->count; i++) {
        cache->entries[i].inUse = FALSE;
        cache->entries[i].wanted = 0;
    }
}

// The animation `animID` if it is cached, for a puppet to be drawn with while it waits for
// `wantedID`, which is already validated. It is not counted as played.
static struct Animation *anim_cache_hold(s32 animID, s32 wantedID) {
    struct AnimCache *cache = &sPrivate->animCache;
    s32 i;

    for (i = 0; i < cache->count; i++) {
        if (cache->entries[i].animID == animID) {
            cache->entries[i].wanted = wantedID + 1;
            return cache->entries[i].anim;
        }
    }
    return NULL;
}

// Returns NULL if the animation does not fit this frame; room is made at the start of the next.
static struct Animation *anim_cache_get(s32 animID) {
    struct AnimCache *cache = &sPrivate->animCache;
    struct Animation *anim;
    u32 size = anim_size(animID);
    u32 start = anim_cache_align(cache->used);
    s32 i;

    for (i = 0; i < cache->count; i++) {
        if (cache->entries[i].animID == animID) {
            cache->entries[i].inUse = TRUE;
            return cache->entries[i].anim;
        }
    }

    // Entries the Mario table leaves empty never load.
    if (size == 0 || size > ANIM_CACHE_SIZE) {
        return NULL;
    }
    if (cache->count >= ANIM_CACHE_ENTRIES || start + size > ANIM_CACHE_SIZE) {
        cache->compactPending = TRUE;
        return NULL;
    }

    anim = (struct Animation *) (MP_ANIM_CACHE_ADDR + start);
    cache->list.dmaTable = gMarioState->animList->dmaTable;
    cache->list.bufTarget = anim;
    cache->list.currentAddr = NULL; // Forces the DMA: this destination is always fresh.
    if (!mario_anim_load_patchable_table(&cache->list, animID)) {
        return NULL;
    }
    // A loaded animation holds offsets from its own start; the renderer expects pointers.
    anim->values = (void *) VIRTUAL_TO_PHYSICAL((u8 *) anim + (u32) anim->values);
    anim->index = (void *) VIRTUAL_TO_PHYSICAL((u8 *) anim + (u32) anim->index);

    cache->entries[cache->count].animID = animID;
    cache->entries[cache->count].inUse = TRUE;
    cache->entries[cache->count].wanted = 0;
    cache->entries[cache->count].anim = anim;
    cache->count++;
    cache->used = start + size;
    return anim;
}

// Copies the fields the game uses. Every access is volatile, so none moves across the sequence reads.
static void copy_slot_fields(volatile struct MpPlayerSlot *to, volatile struct MpPlayerSlot *from) {
    to->active = from->active;
    to->areaIndex = from->areaIndex;
    to->levelNum = from->levelNum;
    to->pos[0] = from->pos[0];
    to->pos[1] = from->pos[1];
    to->pos[2] = from->pos[2];
    to->angle[0] = from->angle[0];
    to->angle[1] = from->angle[1];
    to->angle[2] = from->angle[2];
    to->animID = from->animID;
    to->animFrame = from->animFrame;
    to->platform = from->platform;
    to->platformOffset[0] = from->platformOffset[0];
    to->platformOffset[1] = from->platformOffset[1];
    to->platformOffset[2] = from->platformOffset[2];
}

// Where `pos` is on `platform`: from the platform's place, turned back by the way it faces, so it
// holds on a platform that turns. Tilt is left out: it moves a rider little.
static void to_platform(Vec3s offset, struct Object *platform, Vec3f pos) {
    f32 dx = pos[0] - platform->oPosX;
    f32 dz = pos[2] - platform->oPosZ;
    f32 sine = sins(platform->oFaceAngleYaw);
    f32 cosine = coss(platform->oFaceAngleYaw);

    offset[0] = mp_saturate_s16(dx * cosine - dz * sine);
    offset[1] = mp_saturate_s16(pos[1] - platform->oPosY);
    offset[2] = mp_saturate_s16(dz * cosine + dx * sine);
}

// The place `offset` on this game's `platform`: the reverse of to_platform.
static void from_platform(Vec3f pos, struct Object *platform, Vec3s offset) {
    f32 sine = sins(platform->oFaceAngleYaw);
    f32 cosine = coss(platform->oFaceAngleYaw);

    pos[0] = platform->oPosX + offset[0] * cosine + offset[2] * sine;
    pos[1] = platform->oPosY + offset[1];
    pos[2] = platform->oPosZ + offset[2] * cosine - offset[0] * sine;
}

// Returns FALSE, leaving `copy` unchanged, if the bridge was writing the slot.
static s32 read_remote_slot(struct MpPlayerSlot *copy, volatile struct MpPlayerSlot *slot) {
    struct MpPlayerSlot fresh;
    u32 sequence = slot->sequence;

    if (sequence & 1) {
        return FALSE;
    }
    copy_slot_fields(&fresh, slot);
    if (slot->sequence != sequence) {
        return FALSE;
    }
    *copy = fresh;
    return TRUE;
}

// One frame of a puppet whose player is no longer here to draw, or has nothing to be drawn with:
// it fades where it last stood, in its last pose, and is then hidden, and removed if its player
// has left the session.
static void fade_out(struct Object *puppet, s32 hasLeft) {
    struct GraphNodeObject *gfx = &puppet->header.gfx;
    struct Animation *anim = NULL;

    if ((gfx->node.flags & GRAPH_RENDER_ACTIVE) && puppet->oOpacity > FADE_STEP) {
        // Its last animation must stay cached for as long as it is drawn with it.
        anim = anim_cache_get(gfx->animInfo.animID);
    }
    if (anim == NULL) {
        gfx->node.flags &= ~GRAPH_RENDER_ACTIVE;
        puppet->oOpacity = 0;
        if (hasLeft) {
            obj_mark_for_deletion(puppet);
        }
        return;
    }
    gfx->animInfo.curAnim = anim;
    puppet->oOpacity -= FADE_STEP;
}

void mp_puppet_update(void) {
    struct Object *puppet = gCurrentObject;
    s32 index = puppet->oBehParams2ndByte;
    struct MpPlayerSlot *slot = &sPrivate->puppets[index].slot;
    struct GraphNodeObject *gfx = &puppet->header.gfx;
    struct Animation *anim = NULL;
    struct Object *platform;
    s32 wasDrawn = gfx->node.flags & GRAPH_RENDER_ACTIVE;
    s32 isNewAnim = TRUE;

    // On a torn read, showing the last consistent state for one more frame is harmless.
    read_remote_slot(slot, &gMpMailbox->players[index]);

    if (slot->active && slot->levelNum == gCurrLevelNum && slot->areaIndex == gCurrAreaIndex
        && is_slot_valid(slot)) {
        anim = anim_cache_get(slot->animID);
        if (anim == NULL && wasDrawn) {
            // No room for the new animation yet: the puppet keeps the one it was drawn with, if
            // that is still cached, rather than vanish. That one is not counted as played, so it
            // is dropped if that makes the room.
            anim = anim_cache_hold(gfx->animInfo.animID, slot->animID);
            isNewAnim = FALSE;
        }
    }
    if (anim == NULL) {
        fade_out(puppet, !slot->active);
        return;
    }
    // Fades in from nothing when it appears, rather than popping.
    if (!wasDrawn) {
        puppet->oOpacity = 0;
    }
    puppet->oOpacity = puppet->oOpacity + FADE_STEP > OPAQUE ? OPAQUE : puppet->oOpacity + FADE_STEP;

    gfx->sharedChild = gMarioObject->header.gfx.sharedChild;
    gfx->node.flags |= GRAPH_RENDER_ACTIVE;
    // The frame comes from the mailbox, so the renderer must not advance it as well.
    gfx->node.flags &= ~GRAPH_RENDER_HAS_ANIMATION;

    // On this game's copy of what the player stands on, if it has one; else where they are.
    platform = mp_numbered_object(slot->platform);
    if (platform != NULL) {
        from_platform(slot->pos, platform, slot->platformOffset);
    }
    vec3f_copy(gfx->pos, slot->pos);
    vec3s_copy(gfx->angle, slot->angle);
    vec3f_copy(gfx->scale, gMarioObject->header.gfx.scale);
    vec3f_copy(&puppet->oPosX, slot->pos);
    // An object this game runs may see this player as Mario; see mp_cur_obj_update.
    puppet->oFaceAnglePitch = slot->angle[0];
    puppet->oFaceAngleYaw = slot->angle[1];
    puppet->oFaceAngleRoll = slot->angle[2];
    // Another player is as big as the local one; push_out_of_puppets reads these.
    puppet->hitboxRadius = gMarioObject->hitboxRadius;
    puppet->hitboxHeight = gMarioObject->hitboxHeight;

    gfx->animInfo.curAnim = anim;
    gfx->animInfo.animAccel = 0;
    gfx->animInfo.animYTrans = gMarioObject->header.gfx.animInfo.animYTrans;
    if (isNewAnim) {
        gfx->animInfo.animID = slot->animID;
        // A negative frame would index before the animation's data.
        gfx->animInfo.animFrame = slot->animFrame < 0 ? 0 : slot->animFrame;
    }
}

static void publish_local_player(void) {
    volatile struct MpPlayerSlot *local = &gMpMailbox->local;
    struct GraphNodeObject *gfx = &gMarioObject->header.gfx;
    struct MpPlayerSlot fresh;

    fresh.active = TRUE;
    fresh.areaIndex = gCurrAreaIndex;
    fresh.levelNum = gCurrLevelNum;
    vec3f_copy(fresh.pos, gfx->pos);
    vec3s_copy(fresh.angle, gfx->angle);
    fresh.animID = gfx->animInfo.animID;
    fresh.animFrame = gfx->animInfo.animFrame;
    fresh.platform = gMarioObject->platform != NULL ? mp_object_id(gMarioObject->platform) : 0;
    fresh.platformOffset[0] = fresh.platformOffset[1] = fresh.platformOffset[2] = 0;
    if (fresh.platform != 0) {
        to_platform(fresh.platformOffset, gMarioObject->platform, fresh.pos);
    }

    local->sequence = local->sequence | 1;
    copy_slot_fields(local, &fresh);
    local->sequence = local->sequence + 1;
}

// Forgets a puppet whose object is gone, and returns the puppet's object if it still has one.
static struct Object *live_puppet(struct PuppetRuntime *runtime) {
    // Pool slots are reused, and area changes unload objects, so an old pointer proves nothing.
    if (runtime->object != NULL
        && (runtime->object->activeFlags == ACTIVE_FLAG_DEACTIVATED
            || runtime->object->behavior != bhvMpPuppet)) {
        runtime->object = NULL;
    }
    return runtime->object;
}

// The puppet in remote slot `slot` if it is drawn here. Hidden puppets are in another area or hold
// invalid data, so they have nowhere to be.
static struct Object *visible_puppet(s32 slot) {
    struct Object *puppet = live_puppet(&sPrivate->puppets[slot]);

    if (puppet == NULL || !(puppet->header.gfx.node.flags & GRAPH_RENDER_ACTIVE)) {
        return NULL;
    }
    return puppet;
}

struct Object *mp_visible_puppet(s32 slot) {
    return visible_puppet(slot);
}

f32 mp_camera_fov(void) {
    return sPrivate->fov;
}

typedef Gfx *(*GeoFunction)(s32 callContext, struct GraphNode *node, Mat4 *mtx);

// Mario's model takes its opacity from one state that every Mario drawn shares. While a puppet
// that is fading is drawn, that state says he is see-through, and by how much, as the vanish cap
// does; it is put back at once, so the local Mario and the other puppets are drawn as they are.
static Gfx *with_puppet_opacity(GeoFunction function, s32 callContext, struct GraphNode *node,
                                Mat4 *mtx) {
    struct MarioBodyState *body = gMarioState->marioBodyState;
    struct Object *drawn = (struct Object *) gCurGraphNodeObject;
    s16 own;
    Gfx *gfx;

    // The model also calls these as it is built, at boot, before Mario has a state; and only
    // objects in the pool have a behavior to ask: the mirror's Mario, for one, is not one.
    if (callContext != GEO_CONTEXT_RENDER || body == NULL || drawn < gObjectPool
        || drawn >= gObjectPool + OBJECT_POOL_CAPACITY || drawn->behavior != bhvMpPuppet
        || drawn->oOpacity >= OPAQUE) {
        return function(callContext, node, mtx);
    }
    own = body->modelState;
    body->modelState = MODEL_STATE_ALPHA | drawn->oOpacity;
    gfx = function(callContext, node, mtx);
    body->modelState = own;
    return gfx;
}

// The outfits a player chooses from: the color of the cap and shirt, then of the overalls.
// The first is the game's own red and blue. The bridge and the panel have the same colors
// under names, `OUTFITS` in protocol/src/chat.rs, where a test keeps the two in step.
static const u8 sPlayerColors[MP_PLAYER_SLOTS][6] = {
    { 0xFF, 0x00, 0x00, 0x00, 0x00, 0xFF }, { 0x00, 0xC0, 0x20, 0x20, 0x20, 0xA0 },
    { 0xFF, 0xD0, 0x00, 0x70, 0x00, 0xA0 }, { 0x80, 0x00, 0xC0, 0x20, 0x20, 0x20 },
    { 0xFF, 0x70, 0x00, 0x00, 0x60, 0x60 }, { 0x00, 0xC0, 0xE0, 0xC0, 0x30, 0x00 },
    { 0xFF, 0x60, 0xB0, 0x60, 0x20, 0x80 }, { 0xF0, 0xF0, 0xF0, 0xC0, 0x00, 0x00 },
    { 0x30, 0x30, 0x30, 0xD0, 0xD0, 0xD0 }, { 0x80, 0x48, 0x10, 0x00, 0x70, 0x20 },
    { 0xA0, 0xFF, 0x00, 0x00, 0x40, 0x90 }, { 0x00, 0x30, 0xC0, 0xE0, 0xC0, 0x00 },
    { 0xE0, 0x00, 0xA0, 0x00, 0x90, 0x90 }, { 0x00, 0x90, 0x80, 0x70, 0x30, 0x00 },
    { 0xC0, 0x98, 0x00, 0x50, 0x50, 0x50 }, { 0x80, 0x80, 0x90, 0x90, 0x00, 0x40 },
};

// Multiplayer/mario_colors.asm points the model's colored parts at this segment: overalls at its
// start, then the cap and shirt of the model with red ones, then of the one with green.
#define COLOR_SEGMENT 0x0D
#define LIGHT_BYTES 0x18
#define COLOR_LIGHTS 3

// Writes the game's `Lights1` for a part of `color`: its shade, the color, and where the
// light comes from, as the model's own.
static void put_light(u8 *light, const u8 *color) {
    s32 i;

    for (i = 0; i < 3; i++) {
        light[i] = light[4 + i] = color[i] >> 2;
        light[8 + i] = light[12 + i] = color[i];
        light[16 + i] = 0x28;
    }
}

// Mario's model takes its colors from memory every Mario drawn shares. Its cap, shirt and
// overalls read them through a segment instead, which this sets before each Mario is drawn:
// to the outfit that Mario's player chose, as the bridge tells it, or to the model's own
// colors outside a session.
static Gfx *with_player_colors(Gfx *gfx) {
    static const u8 own[] = { 0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0xFF, 0x00 };
    struct Object *drawn = (struct Object *) gCurGraphNodeObject;
    const u8 *overalls = &own[0], *red = &own[3], *green = &own[6];
    // The local Mario's, and the mirror's, unless this is a puppet.
    s32 who = gMpMailbox->people.localRole;
    Gfx *list;
    u8 *lights;
    s32 slot;

    if (gfx == NULL) {
        return NULL;
    }
    list = alloc_display_list(2 * sizeof(Gfx) + COLOR_LIGHTS * LIGHT_BYTES);
    if (list == NULL) {
        return gfx;
    }
    for (slot = 0; slot < MP_PUPPETS; slot++) {
        if (sPrivate->puppets[slot].object == drawn) {
            who = gMpMailbox->people.roles[slot];
            break;
        }
    }
    if (who & MP_PERSON_DRESSED) {
        red = green = sPlayerColors[who >> MP_PERSON_OUTFIT_SHIFT];
        overalls = red + 3;
    }
    lights = (u8 *) &list[2];
    bzero(lights, COLOR_LIGHTS * LIGHT_BYTES);
    put_light(lights, overalls);
    put_light(lights + LIGHT_BYTES, red);
    put_light(lights + 2 * LIGHT_BYTES, green);
    gSPSegment(&list[0], COLOR_SEGMENT, VIRTUAL_TO_PHYSICAL(lights));
    gSPBranchList(&list[1], gfx);
    return list;
}

// These two stand in for the game's own in Mario's model, where it reads that state: once for
// how see-through to draw him, and once for which of his bodies to draw. See segment.asm.
Gfx *mp_mario_set_alpha(s32 callContext, struct GraphNode *node, Mat4 *mtx) {
    if (gCurGraphNodeCamFrustum != NULL) {
        sPrivate->fov = gCurGraphNodeCamFrustum->fov;
    }
    return with_player_colors(
        with_puppet_opacity(geo_mirror_mario_set_alpha, callContext, node, mtx));
}

Gfx *mp_mario_cap_effect(s32 callContext, struct GraphNode *node, Mat4 *mtx) {
    return with_puppet_opacity(geo_switch_mario_cap_effect, callContext, node, mtx);
}

// Whether Mario's and a puppet's cylinders overlap in height, as in the game's own collisions.
static s32 overlaps_in_height(struct MarioState *m, struct Object *puppet) {
    f32 height = puppet->oPosY - m->pos[1];

    return height < m->marioObj->hitboxHeight && -height < puppet->hitboxHeight;
}

// Bumping: moves the local Mario out of any other player he overlaps. Each game moves only its own
// Mario, so a bump needs no message, adds no delay, and the network never moves anyone's Mario.
// Both games see the overlap, so both players are pushed apart, each by their own game.
static void push_out_of_puppets(void) {
    struct MarioState *m = gMarioState;
    struct Object *puppet;
    s32 i;

    if (m->action & ACT_FLAG_INTANGIBLE) {
        return;
    }
    for (i = 0; i < MP_PUPPETS; i++) {
        puppet = visible_puppet(i);
        if (puppet != NULL && overlaps_in_height(m, puppet)) {
            // Moves Mario sideways to the edge of the puppet, unless a wall or the lack of a
            // floor forbids it.
            push_mario_out_of_object(m, puppet, 0.0f);
        }
    }
}

// Tells the bridge that the local player hit the player in remote slot `slot`.
static void post_hit(s32 slot, s32 kind) {
    volatile struct MpHitRing *out = &gMpMailbox->hitsOut;
    u32 count = out->count;

    out->entries[count % MP_HIT_ENTRIES].slot = slot;
    out->entries[count % MP_HIT_ENTRIES].kind = kind;
    // Last, so the bridge never reads a half-written entry.
    out->count = count + 1;
}

// Hits by the local Mario: during the frames his punch, kick or trip is out, each other player in
// reach and in front of him is hit once. The victim's game decides what the hit does.
static void land_hits(void) {
    struct MarioState *m = gMarioState;
    u32 attack = m->flags & (MARIO_PUNCHING | MARIO_KICKING | MARIO_TRIPPING);
    s32 kind = MP_HIT_PUNCH;
    s16 arc = STRIKE_ARC;
    struct Object *puppet;
    f32 dx;
    f32 dz;
    f32 reach;
    s16 offAngle;
    s32 i;

    if (m->flags & MARIO_KICKING) {
        kind = MP_HIT_KICK;
    }
    if (m->flags & MARIO_TRIPPING) {
        kind = MP_HIT_TRIP;
        arc = TRIP_ARC;
    }
    for (i = 0; i < MP_PUPPETS; i++) {
        if (!attack) {
            // The swing is over, so the next one may land again.
            sPrivate->swingLanded[i] = FALSE;
            continue;
        }
        puppet = visible_puppet(i);
        if (puppet == NULL || sPrivate->swingLanded[i] || !overlaps_in_height(m, puppet)) {
            continue;
        }
        dx = puppet->oPosX - m->pos[0];
        dz = puppet->oPosZ - m->pos[2];
        reach = m->marioObj->hitboxRadius + puppet->hitboxRadius + HIT_REACH;
        // The game's own angle from Mario to an object, against the way he faces.
        offAngle = (s16) (atan2s(dz, dx) - m->faceAngle[1]);
        if (dx * dx + dz * dz <= reach * reach && offAngle >= -arc && offAngle <= arc) {
            post_hit(i, kind);
            sPrivate->swingLanded[i] = TRUE;
        }
    }
}

// Knocks the local Mario back from the player in remote slot `slot`, as the game's own
// take_damage_and_knock_back does for an enemy, unless he cannot be hurt right now.
static void take_hit(u32 slot, u32 kind) {
    struct MarioState *m = gMarioState;
    struct Object *attacker;
    s32 damage;

    // Both come from the network through the bridge.
    if (slot >= MP_PUPPETS || kind < MP_HIT_PUNCH || kind > MP_HIT_TRIP) {
        return;
    }
    attacker = visible_puppet(slot);
    if (attacker == NULL || m->invincTimer != 0
        || (m->action & (ACT_FLAG_INTANGIBLE | ACT_FLAG_INVULNERABLE))
        || (m->flags & MARIO_VANISH_CAP)) {
        return;
    }
    // The knockback reads the damage and the direction from the object that hit Mario.
    attacker->oDamageOrCoinValue = HIT_DAMAGE;
    attacker->oInteractionSubtype = 0;
    m->interactObj = attacker;
    damage = take_damage_from_interact_object(m);
    play_sound(SOUND_MARIO_ATTACKED, m->marioObj->header.gfx.cameraToObject);
    drop_and_set_mario_action(m, determine_knockback_action(m, attacker->oDamageOrCoinValue),
                              damage);
}

// Takes the hits the bridge added to `hitsIn` since the last frame.
static void take_hits(void) {
    volatile struct MpHitRing *in = &gMpMailbox->hitsIn;
    u32 count = in->count;
    u32 behind = count - sPrivate->hitsInTaken;
    u32 index;

    if (behind > MP_HIT_ENTRIES) {
        // Older entries were overwritten; or the count went backwards, so start again from it.
        sPrivate->hitsInTaken = behind < 0x80000000 ? count - MP_HIT_ENTRIES : count;
    }
    while (sPrivate->hitsInTaken != count) {
        index = sPrivate->hitsInTaken % MP_HIT_ENTRIES;
        sPrivate->hitsInTaken++;
        take_hit(in->entries[index].slot, in->entries[index].kind);
    }
}

// Runs as Mario's behavior, so it only runs when objects do: never while paused or between areas.
// Mario's object list updates before the puppets', so this runs first in every frame.
void mp_mario_update(void) {
    s32 i;

    // Before Mario's own update, so his step starts from a knockback or where a bump left him,
    // among objects where their owners have them.
    take_hits();
    mp_take_object_states();
    mp_take_progress();
    push_out_of_puppets();

    bhv_mario_update();

    // After it: his attack flags show what his action did this frame, and the objects he touched
    // hold what he did to them.
    land_hits();
    mp_claim_touched_objects();
    mp_report_coins();

    publish_local_player();
    mp_publish_objects();
    mp_publish_progress();

    anim_cache_begin_frame();

    for (i = 0; i < MP_PUPPETS; i++) {
        struct PuppetRuntime *runtime = &sPrivate->puppets[i];

        if (gMpMailbox->players[i].active && live_puppet(runtime) == NULL) {
            // Unlike spawn_object, this returns NULL instead of hanging when the pool is full.
            runtime->object = try_to_spawn_object(0, 1.0f, gMarioObject, MODEL_NONE, bhvMpPuppet);
            if (runtime->object != NULL) {
                runtime->object->oBehParams2ndByte = i;
                // Another player must not freeze because the local player opened a text box.
                runtime->object->activeFlags |= ACTIVE_FLAG_INITIATED_TIME_STOP;
            }
        }
    }
}
