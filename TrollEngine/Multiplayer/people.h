#ifndef MULTIPLAYER_PEOPLE_H
#define MULTIPLAYER_PEOPLE_H

#include <PR/ultratypes.h>
#include "types.h"

// In a multiplayer build, the game's HUD is followed by the other players' names, the chat and
// the player list (Multiplayer/people.c). MP_HUD(render_hud)() calls the HUD through its wrapper at
// no extra size where it is called; without MULTIPLAYER it is the plain call, keeping the
// single-player build byte-identical.
#ifdef MULTIPLAYER

void mp_render_hud(void);
// Multiplayer/puppet.c: the puppet in remote slot `slot` if it is drawn, and the camera's field of
// view, in degrees, as of the last Mario drawn.
struct Object *mp_visible_puppet(s32 slot);
f32 mp_camera_fov(void);

#define MP_HUD(function) mp_##function

#else

#define MP_HUD(function) function

#endif

#endif
