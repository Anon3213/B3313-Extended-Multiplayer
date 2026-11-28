#include <PR/ultratypes.h>
#include <PR/gbi.h>

#include "segment2.h"

#include "Multiplayer/font.h"

// The display lists text is drawn between (Multiplayer/people.c). They are data, and sit with
// the font in the second segment, as the first has no room for them; see segment.asm.

// Sets the screen up for text and loads the font, whole, as the one texture it is, with its
// two palettes, which turn a pixel into brightness and opacity. Text is tinted by the
// primitive color, so a glyph's bright pixels take it and its dark outline stays dark; each
// pixel of the texture is one of the screen, unsmoothed. The render tile shows the font's first
// image, and the tile after it the second (Multiplayer/font.h).
const Gfx gMpTextBegin[] = {
    gsDPPipeSync(),
    gsDPSetCycleType(G_CYC_1CYCLE),
    gsDPSetTexturePersp(G_TP_NONE),
    gsDPSetTextureFilter(G_TF_POINT),
    gsDPSetCombineMode(G_CC_MODULATEIA_PRIM, G_CC_MODULATEIA_PRIM),
    gsDPSetRenderMode(G_RM_XLU_SURF, G_RM_XLU_SURF2),
    gsDPSetTextureLUT(G_TT_IA16),
    gsDPLoadTLUT(32, 256, FONT_PALETTES),
    gsDPLoadTextureBlock_4b(FONT_TEXTURE, G_IM_FMT_CI, FONT_WIDTH, FONT_HEIGHT, 0, G_TX_CLAMP,
                            G_TX_CLAMP, G_TX_NOMASK, G_TX_NOMASK, G_TX_NOLOD, G_TX_NOLOD),
    gsDPSetTile(G_IM_FMT_CI, G_IM_SIZ_4b, ((FONT_WIDTH >> 1) + 7) >> 3, 0, G_TX_RENDERTILE + 1, 1,
                G_TX_CLAMP, G_TX_NOMASK, G_TX_NOLOD, G_TX_CLAMP, G_TX_NOMASK, G_TX_NOLOD),
    gsDPSetTileSize(G_TX_RENDERTILE + 1, 0, 0, (FONT_WIDTH - 1) << G_TEXTURE_IMAGE_FRAC,
                    (FONT_HEIGHT - 1) << G_TEXTURE_IMAGE_FRAC),
    gsSPEndDisplayList(),
};

// Puts back what the game's own text routine leaves, which whatever is drawn next expects:
// textures with no palette among it.
const Gfx gMpTextEnd[] = {
    gsDPPipeSync(),
    gsDPSetTexturePersp(G_TP_PERSP),
    gsDPSetTextureLUT(G_TT_NONE),
    gsSPBranchList(dl_ia_text_end),
};
