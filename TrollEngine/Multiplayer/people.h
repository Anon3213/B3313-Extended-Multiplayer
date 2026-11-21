#ifndef MULTIPLAYER_PEOPLE_H
#define MULTIPLAYER_PEOPLE_H

#include <PR/ultratypes.h>
#include "types.h"

// The game's HUD is followed by the other players' names, the chat and the player list
// (Multiplayer/people.c): render_game calls mp_render_hud where it called render_hud
// (Multiplayer/hooks.asm).
void mp_render_hud(void);
// Multiplayer/puppet.c: the puppet in remote slot `slot` if it is drawn, and the camera's field of
// view, in degrees, as of the last Mario drawn.
struct Object *mp_visible_puppet(s32 slot);
f32 mp_camera_fov(void);

#endif
