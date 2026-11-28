#include <PR/ultratypes.h>
#include <PR/gbi.h>

#include "config.h"

#include "Multiplayer/mailbox.h"
#include "Multiplayer/map.h"

// The minimap, which the bridge draws from the area's walls and where the players are
// (bridge/src/minimap.rs), and the game only shows: one picture of 64 by 64 pixels and its
// palette, both at MP_MAP_ADDR (struct MpMap), as one textured rectangle. That is all the game
// does, so the display list is data, in the second segment, and costs the first segment's code
// only the call (mp_render_hud in Multiplayer/people.c).
//
// A picture with a palette leaves the texture memory's upper half to the palette, so 2 KB of
// picture is the most there is room for: 64 by 64 pixels at four bits.

// The lower right corner, above the camera's mark.
#define MAP_LEFT (SCREEN_WIDTH - 8 - MP_MAP_SIDE)
#define MAP_TOP (SCREEN_HEIGHT - 40 - MP_MAP_SIDE)
// How opaque the map is: the palette's colors, all shown at this alpha, or not at all.
#define MAP_ALPHA 200
// The pixel's color from the palette, and its alpha, which the palette has as all or nothing,
// times MAP_ALPHA.
#define G_CC_MP_MAP 0, 0, 0, TEXEL0, TEXEL0, 0, PRIMITIVE, 0
// Where the palette is, after the pixels (struct MpMap).
#define MAP_PALETTE (MP_MAP_ADDR + MP_MAP_SIDE * MP_MAP_SIDE / 2)

const Gfx gMpMapDraw[] = {
    gsDPPipeSync(),
    gsDPSetCycleType(G_CYC_1CYCLE),
    gsDPSetTexturePersp(G_TP_NONE),
    gsDPSetTextureFilter(G_TF_POINT),
    gsDPSetCombineMode(G_CC_MP_MAP, G_CC_MP_MAP),
    gsDPSetRenderMode(G_RM_XLU_SURF, G_RM_XLU_SURF2),
    gsDPSetPrimColor(0, 0, 255, 255, 255, MAP_ALPHA),
    gsDPSetTextureLUT(G_TT_RGBA16),
    gsDPLoadTLUT_pal16(0, MAP_PALETTE),
    gsDPLoadTextureBlock_4b(MP_MAP_ADDR, G_IM_FMT_CI, MP_MAP_SIDE, MP_MAP_SIDE, 0, G_TX_CLAMP,
                            G_TX_CLAMP, G_TX_NOMASK, G_TX_NOMASK, G_TX_NOLOD, G_TX_NOLOD),
    gsSPTextureRectangle(MAP_LEFT << 2, MAP_TOP << 2, (MAP_LEFT + MP_MAP_SIDE) << 2,
                         (MAP_TOP + MP_MAP_SIDE) << 2, G_TX_RENDERTILE, 0, 0, 1 << 10, 1 << 10),
    // Whatever is drawn next takes its textures as they are, with no palette.
    gsDPPipeSync(),
    gsDPSetTextureLUT(G_TT_NONE),
    gsSPEndDisplayList(),
};
