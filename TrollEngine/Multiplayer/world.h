#ifndef MULTIPLAYER_WORLD_H
#define MULTIPLAYER_WORLD_H

#include <PR/ultratypes.h>

// B3313 personalizes each level from its player's trackers, seed, stars and clock. Between
// mp_world_begin and mp_world_end the session's values are swapped in, so every player
// generates the same level. Multiplayer/hooks.asm points each of the game's calls to a
// generation step at the function here that runs it that way.

void mp_world_begin(void);
void mp_world_end(void);

// Called where the game called the step each is named for.
void mp_post_object_load_pass(void);
void mp_level_init_intercept(void);
void mp_area_geo_load_intercept(u8 areaId, u32 *areaGeoLayout);
void mp_addMoreObjects(void);
void mp_load_stuff_in_level_pool(void);
void mp_intercept_bg_seg_load(u8 **segStart, u8 **segEnd);
// Called where troll_geo_layout called save_file_get_total_star_count.
s32 mp_geo_total_stars(s32 fileIndex, s32 minCourse, s32 maxCourse);
// Takes the place of the engine's get_red_star_count.
s32 mp_get_red_star_count(s32 fileIndex);
// Run first by updateRTC: whether it set the clock from the session's, leaving updateRTC
// nothing to do.
s32 mp_update_rtc(void);

#endif
