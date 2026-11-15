#include <PR/ultratypes.h>
#include <PR/gbi.h>

#include "segment2.h"

#include "Multiplayer/font.h"

// The display lists text is drawn between (Multiplayer/people.c). They are data, and sit with
// the font in the second segment, as the first has no room for them; see segment.asm.

// Sets the screen up for text and loads the font, whole, as the one texture it is. Text is
// tinted by the primitive color, so a glyph's bright pixels take it and its dark outline
// stays dark; each pixel of the texture is one of the screen, unsmoothed.
const Gfx gMpTextBegin[] = {
    gsDPPipeSync(),
    gsDPSetCycleType(G_CYC_1CYCLE),
    gsDPSetTexturePersp(G_TP_NONE),
    gsDPSetTextureFilter(G_TF_POINT),
    gsDPSetCombineMode(G_CC_MODULATEIA_PRIM, G_CC_MODULATEIA_PRIM),
    gsDPSetRenderMode(G_RM_XLU_SURF, G_RM_XLU_SURF2),
    gsDPLoadTextureBlock_4b(FONT_TEXTURE, G_IM_FMT_IA, FONT_COLUMNS * CELL_WIDTH,
                            FONT_CELLS / FONT_COLUMNS * CELL_HEIGHT, 0, G_TX_CLAMP, G_TX_CLAMP,
                            G_TX_NOMASK, G_TX_NOMASK, G_TX_NOLOD, G_TX_NOLOD),
    gsSPEndDisplayList(),
};

// Puts back what the game's own text routine leaves, which whatever is drawn next expects.
const Gfx gMpTextEnd[] = {
    gsDPPipeSync(),
    gsDPSetTexturePersp(G_TP_PERSP),
    gsSPBranchList(dl_ia_text_end),
};
