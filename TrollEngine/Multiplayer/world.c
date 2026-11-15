#include "sm64.h"
#include "area.h"
#include "level_update.h"
#include "save_file.h"
#include "course_table.h"

#include "AI/personalization_helpers.h"
#include "AI/post_object_load_pass.h"
#include "AI/stats_tracking.h"
#include "Multiplayer/mailbox.h"
#include "Multiplayer/objects.h"
#include "Multiplayer/world.h"

// Every tracker global in AI/stats_tracking.h, scalars as SCALAR and arrays as ARRAY. Keep this in
// step with that header: generation reads them, so a missing one would differ between players.
#define TRACKER_FLOATS(SCALAR, ARRAY)                       \
    SCALAR(TRACKER_speed)                                   \
    SCALAR(TRACKER_air_time)                                \
    SCALAR(TRACKER_water)                                   \
    SCALAR(TRACKER_hp)                                      \
    SCALAR(TRACKER_boss_performance)                        \
    SCALAR(TRACKER_accum_deaths)                            \
    SCALAR(TRACKER_accum_deaths_enemy_knockback)            \
    SCALAR(TRACKER_accum_deaths_skill_issue)                \
    SCALAR(TRACKER_accum_deaths_fire_lava)                  \
    SCALAR(TRACKER_accum_deaths_death_barrier)              \
    SCALAR(TRACKER_accum_deaths_trolled)                    \
    ARRAY(TRACKER_death_reasons, 5)                         \
    SCALAR(TRACKER_accum_social)                            \
    SCALAR(TRACKER_accum_nerd)                              \
    SCALAR(TRACKER_accum_murder)                            \
    SCALAR(TRACKER_accum_stars)                             \
    SCALAR(TRACKER_accum_stars_prefer_boss)                 \
    SCALAR(TRACKER_accum_stars_prefer_cannon)               \
    SCALAR(TRACKER_accum_stars_prefer_collect)              \
    SCALAR(TRACKER_accum_stars_prefer_free)                 \
    SCALAR(TRACKER_accum_stars_prefer_murder)               \
    SCALAR(TRACKER_accum_stars_prefer_platform)             \
    SCALAR(TRACKER_accum_stars_prefer_slide)                \
    SCALAR(TRACKER_accum_stars_prefer_timed)                \
    ARRAY(TRACKER_star_preferences_gameplay, 8)             \
    SCALAR(TRACKER_accum_stars_prefer_buildings)            \
    SCALAR(TRACKER_accum_stars_prefer_overworld_courses)    \
    SCALAR(TRACKER_accum_stars_prefer_snow_courses)         \
    SCALAR(TRACKER_accum_stars_prefer_desert_courses)       \
    SCALAR(TRACKER_accum_stars_prefer_cave_courses)         \
    SCALAR(TRACKER_accum_stars_prefer_water_courses)        \
    SCALAR(TRACKER_accum_stars_prefer_fire_courses)         \
    SCALAR(TRACKER_accum_stars_prefer_secret_courses)       \
    ARRAY(TRACKER_star_preferences_level_type, 8)           \
    SCALAR(TRACKER_accum_stars_prefer_wing_cap)             \
    SCALAR(TRACKER_accum_stars_prefer_metal_cap)            \
    SCALAR(TRACKER_accum_stars_prefer_vanish_cap)           \
    ARRAY(TRACKER_star_preferences_cap, 3)                  \
    SCALAR(TRACKER_prefer_collect)                          \
    SCALAR(TRACKER_prefer_exploration)                      \
    SCALAR(TRACKER_prefer_lore)                             \
    SCALAR(TRACKER_prefer_murder)                           \
    SCALAR(TRACKER_prefer_parkour)                          \
    SCALAR(TRACKER_prefer_swimming)                         \
    SCALAR(TRACKER_difficulty_modifier)                     \
    SCALAR(TRACKER_level_scale_modifier_h)                  \
    SCALAR(TRACKER_level_scale_modifier_v)                  \
    SCALAR(TRACKER_difficulty_modifier_half)                \
    SCALAR(TRACKER_difficulty_modifier_sqrt_half)

#define COUNT_SCALAR(name) +1
#define COUNT_ARRAY(name, length) +(length)
enum { TRACKER_FLOAT_COUNT = 0 TRACKER_FLOATS(COUNT_SCALAR, COUNT_ARRAY) };
typedef char TrackerCountCheck[TRACKER_FLOAT_COUNT == MP_TRACKER_FLOATS ? 1 : -1];

// The game's own values while the session's are swapped in.
struct WorldStash {
    s32 depth;
    s32 active;
    // The session values that generation reads through overrides, not variables.
    u16 sessionTotalStars;
    u16 sessionRedStars;
    u8 sessionRtcHour;
    u8 sessionRtcMinute;
    u8 sessionRtcSecond;
    f32 trackers[MP_TRACKER_FLOATS];
    u16 trackerFlags;
    u16 personalizationRandSeed;
    u16 randomSeed;
    u8 nightModeSetting;
};

// In RAM the survey found zeroed, so this needs no initialization at boot.
#define sStash ((struct WorldStash *) MP_WORLD_STASH_ADDR)
typedef char WorldStashSizeCheck[sizeof(struct WorldStash) <= MP_OBJECT_IDS_ADDR - MP_WORLD_STASH_ADDR
                                     ? 1
                                     : -1];

extern u16 gRandomSeed16;
extern void addMoreObjects(void);

static void read_trackers(f32 *out) {
    s32 i = 0;
    s32 j;
#define READ_SCALAR(name) out[i++] = name;
#define READ_ARRAY(name, length)     \
    for (j = 0; j < (length); j++) { \
        out[i++] = name[j];          \
    }
    TRACKER_FLOATS(READ_SCALAR, READ_ARRAY)
#undef READ_SCALAR
#undef READ_ARRAY
}

static void write_trackers(f32 *in) {
    s32 i = 0;
    s32 j;
#define WRITE_SCALAR(name) name = in[i++];
#define WRITE_ARRAY(name, length)    \
    for (j = 0; j < (length); j++) { \
        name[j] = in[i++];           \
    }
    TRACKER_FLOATS(WRITE_SCALAR, WRITE_ARRAY)
#undef WRITE_SCALAR
#undef WRITE_ARRAY
}

// Copies every field. Each access is volatile, so none moves across the sequence reads or writes.
static void copy_world(volatile struct MpWorld *to, volatile struct MpWorld *from) {
    s32 i;

    to->valid = from->valid;
    to->nightModeSetting = from->nightModeSetting;
    to->personalizationRandSeed = from->personalizationRandSeed;
    to->trackerFlags = from->trackerFlags;
    to->totalStars = from->totalStars;
    to->redStars = from->redStars;
    to->rtcHour = from->rtcHour;
    to->rtcMinute = from->rtcMinute;
    to->rtcSecond = from->rtcSecond;
    for (i = 0; i < MP_TRACKER_FLOATS; i++) {
        to->trackers[i] = from->trackers[i];
    }
}

// The bridge writes the session world from network data, so nothing in it is trusted.
static s32 read_session_world(struct MpWorld *copy) {
    volatile struct MpWorld *session = &gMpMailbox->sessionWorld;
    u32 sequence = session->sequence;
    s32 i;

    if (sequence & 1) {
        return FALSE;
    }
    copy_world(copy, session);
    if (session->sequence != sequence || !copy->valid || copy->rtcHour >= 24
        || copy->rtcMinute >= 60 || copy->rtcSecond >= 60) {
        return FALSE;
    }
    for (i = 0; i < MP_TRACKER_FLOATS; i++) {
        if (!mp_is_finite(copy->trackers[i])) {
            return FALSE;
        }
    }
    return TRUE;
}

// Publishes the values this game is about to generate with, so a session can adopt them. Generating
// with exactly what is offered keeps the first level and every later one the same.
static void publish_local_world(void) {
    volatile struct MpWorld *local = &gMpMailbox->localWorld;
    struct MpWorld fresh;

    fresh.valid = TRUE;
    fresh.nightModeSetting = nightModeSetting;
    fresh.personalizationRandSeed = personalizationRandSeed;
    fresh.trackerFlags = TRACKER_flags;
    fresh.totalStars =
        save_file_get_total_star_count(gCurrSaveFileNum - 1, COURSE_MIN - 1, COURSE_MAX - 1);
    fresh.redStars = get_red_star_count(gCurrSaveFileNum - 1);
    fresh.rtcHour = rtcHour;
    fresh.rtcMinute = rtcMinute;
    fresh.rtcSecond = rtcSecond;
    read_trackers(fresh.trackers);

    local->sequence = local->sequence | 1;
    copy_world(local, &fresh);
    local->sequence = local->sequence + 1;
}

// Seeds the shared random generator from the world, so generation rolls the same for everyone.
static u16 world_random_seed(u16 seed) {
    return seed ^ (u16) (gCurrLevelNum << 8) ^ (u16) gCurrAreaIndex;
}

void mp_world_begin(void) {
    struct MpWorld session;

    // Generation steps can nest; only the outermost swaps.
    if (sStash->depth++ > 0) {
        return;
    }
    sStash->randomSeed = gRandomSeed16;

    if (!read_session_world(&session)) {
        sStash->active = FALSE;
        publish_local_world();
        gRandomSeed16 = world_random_seed(personalizationRandSeed);
        return;
    }

    read_trackers(sStash->trackers);
    sStash->trackerFlags = TRACKER_flags;
    sStash->personalizationRandSeed = personalizationRandSeed;
    sStash->nightModeSetting = nightModeSetting;
    sStash->sessionTotalStars = session.totalStars;
    sStash->sessionRedStars = session.redStars;
    sStash->sessionRtcHour = session.rtcHour;
    sStash->sessionRtcMinute = session.rtcMinute;
    sStash->sessionRtcSecond = session.rtcSecond;

    write_trackers(session.trackers);
    TRACKER_flags = session.trackerFlags;
    personalizationRandSeed = session.personalizationRandSeed;
    nightModeSetting = session.nightModeSetting;
    gRandomSeed16 = world_random_seed(personalizationRandSeed);
    sStash->active = TRUE;
}

void mp_world_end(void) {
    if (sStash->depth <= 0 || --sStash->depth > 0) {
        return;
    }
    // The game's own values return, so its save keeps its own personality.
    if (sStash->active) {
        write_trackers(sStash->trackers);
        TRACKER_flags = sStash->trackerFlags;
        personalizationRandSeed = sStash->personalizationRandSeed;
        nightModeSetting = sStash->nightModeSetting;
        sStash->active = FALSE;
    }
    gRandomSeed16 = sStash->randomSeed;
}

s32 mp_world_total_stars(s32 own) {
    return sStash->active ? sStash->sessionTotalStars : own;
}

s32 mp_world_red_stars(s32 own) {
    return sStash->active ? sStash->sessionRedStars : own;
}

// The session's clock, fixed when it began, so day or night is the same for everyone.
s32 mp_world_take_rtc(u8 *hour, u8 *minute, u8 *second) {
    if (!sStash->active) {
        return FALSE;
    }
    *hour = sStash->sessionRtcHour;
    *minute = sStash->sessionRtcMinute;
    *second = sStash->sessionRtcSecond;
    return TRUE;
}

// Wrappers for each generation entry point. MP_WORLD renames a call to one of these, so a call site
// costs no more space: some sit in vanilla functions' fixed-size areas.

// load_area calls this instead of postObjectLoadPass; see AI/ai.asm.
void mp_post_object_load_pass(void) {
    MP_WORLD_CALL(postObjectLoadPass());
    mp_number_area_objects();
}

void mp_level_init_intercept(void) {
    MP_WORLD_CALL(level_init_intercept());
}

void mp_area_geo_load_intercept(u8 areaId, u32 *areaGeoLayout) {
    MP_WORLD_CALL(area_geo_load_intercept(areaId, areaGeoLayout));
}

void mp_addMoreObjects(void) {
    MP_WORLD_CALL(addMoreObjects());
}

void mp_load_stuff_in_level_pool(void) {
    MP_WORLD_CALL(load_stuff_in_level_pool());
}
