#ifndef MULTIPLAYER_WORLD_H
#define MULTIPLAYER_WORLD_H

#include <PR/ultratypes.h>

// B3313 personalizes each level from its player's trackers, seed, stars and clock. In a multiplayer
// build, MP_WORLD_CALL runs a level generation step with the session's values swapped in, so every
// player generates the same level. Without MULTIPLAYER, these expand to the plain code, keeping the
// single-player build byte-identical.
#ifdef MULTIPLAYER

void mp_world_begin(void);
void mp_world_end(void);
s32 mp_world_total_stars(s32 own);
s32 mp_world_red_stars(s32 own);
s32 mp_world_take_rtc(u8 *hour, u8 *minute, u8 *second);
void mp_post_object_load_pass(void);
void mp_level_init_intercept(void);
void mp_area_geo_load_intercept(u8 areaId, u32 *areaGeoLayout);
void mp_addMoreObjects(void);
void mp_load_stuff_in_level_pool(void);

// Calls a generation entry point through its wrapper in world.c, at no extra size.
#define MP_WORLD(function) mp_##function

#define MP_WORLD_CALL(call) \
    do {                    \
        mp_world_begin();   \
        call;               \
        mp_world_end();     \
    } while (0)
#define MP_WORLD_TOTAL_STARS(count) mp_world_total_stars(count)
#define MP_WORLD_RED_STARS(count) mp_world_red_stars(count)

#else

#define MP_WORLD(function) function
#define MP_WORLD_CALL(call) call
#define MP_WORLD_TOTAL_STARS(count) (count)
#define MP_WORLD_RED_STARS(count) (count)

#endif

#endif
