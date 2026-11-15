#ifndef MULTIPLAYER_FONT_H
#define MULTIPLAYER_FONT_H

#include <PR/ultratypes.h>
#include <PR/gbi.h>

#include "Multiplayer/mailbox.h"

// The font names and chat are drawn in, as scripts/make_font.py packs it from the drawing in
// Multiplayer/font.txt: each cell's advance, then a texture of 96 cells of 7 by 12 pixels,
// 16 to a row. Cell n is the character 0x20 + n; a space is never drawn and 0x7F is no
// character, so their cells hold the player list's two marks.
#define FONT_ADVANCES ((const u8 *) MP_FONT_ADDR)
#define FONT_CELLS 96
#define FONT_TEXTURE (MP_FONT_ADDR + FONT_CELLS)
#define FONT_COLUMNS 16
#define CELL_WIDTH 7
#define CELL_HEIGHT 12
#define MARK_MODERATOR 0
#define MARK_HOST (FONT_CELLS - 1)

// Multiplayer/font.c: what text is drawn between.
extern const Gfx gMpTextBegin[];
extern const Gfx gMpTextEnd[];

#endif
