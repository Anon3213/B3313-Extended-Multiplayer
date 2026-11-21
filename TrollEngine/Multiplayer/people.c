#include <PR/os_cont.h>
#include <PR/os_libc.h>

#include "sm64.h"
#include "area.h"
#include "engine/graph_node.h"
#include "engine/math_util.h"
#include "game_init.h"
#include "hud.h"
#include "ingame_menu.h"
#include "level_update.h"
#include "object_fields.h"
#include "segment2.h"

#include "Multiplayer/font.h"
#include "Multiplayer/mailbox.h"
#include "Multiplayer/people.h"

// What the other players are to this one on screen: a name over each Mario in view, the newest
// lines of the chat, and, while the D-pad is held down, the list of everyone with where they
// are. While the player types a line of chat, the chat is a box with that line under it.
//
// The bridge writes all text as plain characters. The game draws it in a font of this
// project's own (Multiplayer/font.txt), not B3313's dialog font, whose letters are all
// capitals that run into each other on purpose and which lacks half the marks people type.
// Every glyph has a dark outline, so text reads over any scenery with no box behind it.
//
// The game has no keyboard. The bridge reads the PC's and writes the line as it stands, with
// its prompt and cursor, for the game to draw; all the game does for typing is that, and
// keeping the keys from Mario meanwhile (mp_read_pads).
//
// The whole font is one texture (Multiplayer/font.h), loaded once a frame, and a glyph is one
// rectangle of it: 24 bytes of the frame's display list pool, where the game's dialog routine
// took about 100. Everything here is still counted against the room left before it is drawn,
// and left out if there is not enough: a frame without a name is nothing, and a pool overrun
// is a crash.

// What one glyph costs of the display list pool, and what must stay free after any text, for
// whatever the game still draws this frame.
#define GLYPH_POOL_BYTES 24
#define POOL_MARGIN 6144

#define SCREEN_CENTER_X (SCREEN_WIDTH / 2)
#define SCREEN_CENTER_Y (SCREEN_HEIGHT / 2)
// From one line's top to the next: a cell and a pixel of air.
#define LINE_HEIGHT (CELL_HEIGHT + 1)
// How near the screen's edge text may come.
#define SCREEN_EDGE 2

// Names are drawn for the nearest few Marios in view, within this many units of the camera.
#define NAME_RANGE 4000.0f
#define NAMES_MOST 6
// How far above a Mario's feet his name sits, at his own scale: just over his head, which is
// 160 units up. At 210 the names stood well clear of the heads they belonged to.
#define NAME_HEIGHT 175.0f
#define MIN_DEPTH 100.0f
#define DEFAULT_FOV 45.0f

// The chat is in the lower left corner, clear of the camera's mark in the lower right: its
// rows from CHAT_BOTTOM up, the newest lowest, and under them the row being typed.
#define CHAT_LEFT 12
#define CHAT_BOTTOM 34
// The row being typed, and the strip behind it, which reaches CHAT_PAD above and below the
// row and stops CHAT_GAP short of the rows above. The box they are all in reaches CHAT_SIDE
// past the text at each side.
#define CHAT_PAD 3
#define CHAT_GAP 2
#define CHAT_SIDE 5
#define CHAT_DRAFT_BOTTOM (CHAT_BOTTOM - CHAT_GAP - CHAT_PAD - CELL_HEIGHT)
#define CHAT_BOX_LEFT (CHAT_LEFT - CHAT_SIDE)
#define CHAT_BOX_RIGHT (CHAT_LEFT + MP_CHAT_ROW_PIXELS + CHAT_SIDE)
#define CHAT_BOX_BOTTOM (CHAT_DRAFT_BOTTOM - CHAT_PAD)
#define CHAT_STRIP_TOP (CHAT_BOTTOM - CHAT_GAP)
// The list is held up with the D-pad's down, which neither the game nor B3313 uses in play; L,
// the other spare button, B3313 does use. It is two columns of eight rows, below the HUD's own
// row at the top of the screen, which is drawn after it and would cover it.
#define LIST_BUTTON D_JPAD
#define LIST_LEFT 20
#define LIST_TOP 176
#define LIST_COLUMN_ROWS 8
#define LIST_COLUMN_WIDTH 144
// In a row of the list: where the name starts, after the mark, and the room kept at the
// column's right for where the player is, as "13-1".
#define LIST_NAME_LEFT 9
#define LIST_COLUMN_GAP 8
#define LIST_SHADE_EDGE 6
#define LIST_SHADE_ALPHA 150
// A level and an area of up to three digits each, the dash between them, and the end.
#define PLACE_BYTES 8

// Copies mailbox text of at most `most` characters to `out`, which has room for one more, and
// ends it. A code the font has no glyph for becomes a question mark. Returns its length.
static s32 take_text(u8 *out, volatile u8 *in, s32 most) {
    s32 length;
    u8 code;

    for (length = 0; length < most; length++) {
        code = in[length];
        if (code == MP_TEXT_END) {
            break;
        }
        out[length] = code < ' ' || code > '~' ? '?' : code;
    }
    out[length] = MP_TEXT_END;
    return length;
}

// Whether the frame's display list pool has room for `glyphs` more glyphs of text.
static s32 has_room(s32 glyphs) {
    return gGfxPoolEnd - (u8 *) gDisplayListHead > glyphs * GLYPH_POOL_BYTES + POOL_MARGIN;
}

// Darkens a box of the screen, as the game's own message boxes do: a panel behind the list,
// or the chat while it is typed in. `bottom` and `top` count up from the bottom of the screen,
// as text positions do.
static void shade(s32 left, s32 bottom, s32 right, s32 top) {
    Gfx *gfx = gDisplayListHead;

    gDPPipeSync(gfx++);
    gDPSetCycleType(gfx++, G_CYC_1CYCLE);
    gDPSetCombineMode(gfx++, G_CC_PRIMITIVE, G_CC_PRIMITIVE);
    gDPSetRenderMode(gfx++, G_RM_XLU_SURF, G_RM_XLU_SURF2);
    gDPSetPrimColor(gfx++, 0, 0, 0, 0, 0, LIST_SHADE_ALPHA);
    gDPFillRectangle(gfx++, left, SCREEN_HEIGHT - top, right, SCREEN_HEIGHT - bottom);
    gDisplayListHead = gfx;
}

// Starts text of one color; every draw_text and draw_mark until the next is in it.
static void text_color(s32 red, s32 green, s32 blue) {
    gDPSetPrimColor(gDisplayListHead++, 0, 0, red, green, blue, 255);
}

// Draws one cell of the font with its lower left corner at (x, y) from the bottom left.
static void draw_cell(s32 x, s32 y, s32 cell) {
    s32 top = SCREEN_HEIGHT - y - CELL_HEIGHT;

    gSPTextureRectangle(gDisplayListHead++, x << 2, top << 2, (x + CELL_WIDTH) << 2,
                        (top + CELL_HEIGHT) << 2, G_TX_RENDERTILE,
                        (cell % FONT_COLUMNS * CELL_WIDTH) << 5,
                        (cell / FONT_COLUMNS * CELL_HEIGHT) << 5, 1 << 10, 1 << 10);
}

// Draws one line of text with its lower left corner at (x, y) from the bottom left.
static void draw_text(s32 x, s32 y, const u8 *text) {
    s32 cell;

    for (; *text != MP_TEXT_END; text++) {
        cell = *text - ' ';
        if (cell != 0) {
            draw_cell(x, y, cell);
        }
        x += FONT_ADVANCES[cell];
    }
}

// How wide the font draws `text`, in pixels: its glyphs, and the last one's outline.
static s32 text_width(const u8 *text) {
    s32 width = 1;

    for (; *text != MP_TEXT_END; text++) {
        width += FONT_ADVANCES[*text - ' '];
    }
    return width;
}

// Draws the name of the puppet in `slot` over its head, if it is in view and near enough.
// Returns whether it drew one.
static s32 draw_name(s32 slot, f32 perUnit) {
    struct Object *puppet = mp_visible_puppet(slot);
    u8 name[MP_NAME_BYTES];
    f32 depth;
    s32 width;
    s32 x;
    s32 y;

    if (puppet == NULL) {
        return FALSE;
    }
    // Where the game last drew it, as the camera sees it: x to the right, y up, z behind.
    depth = -puppet->header.gfx.cameraToObject[2];
    if (depth < MIN_DEPTH || depth > NAME_RANGE
        || take_text(name, gMpMailbox->people.names[slot], MP_NAME_BYTES - 1) == 0) {
        return FALSE;
    }
    width = text_width(name);
    x = SCREEN_CENTER_X + (s32) (puppet->header.gfx.cameraToObject[0] * perUnit / depth);
    y = SCREEN_CENTER_Y
        + (s32) ((puppet->header.gfx.cameraToObject[1]
                  + NAME_HEIGHT * puppet->header.gfx.scale[1])
                 * perUnit / depth);
    // Only over a Mario who is himself on the screen.
    if (x < 0 || x > SCREEN_WIDTH || y < SCREEN_EDGE
        || y > SCREEN_HEIGHT - CELL_HEIGHT - SCREEN_EDGE) {
        return FALSE;
    }
    // Centered over him, but kept whole when he stands at the screen's edge.
    x -= width / 2;
    if (x < SCREEN_EDGE) {
        x = SCREEN_EDGE;
    } else if (x + width > SCREEN_WIDTH - SCREEN_EDGE) {
        x = SCREEN_WIDTH - SCREEN_EDGE - width;
    }
    draw_text(x, y, name);
    return TRUE;
}

static void draw_names(void) {
    f32 fov = mp_camera_fov();
    s16 halfFov;
    f32 perUnit;
    s32 drawn = 0;
    s32 slot;

    if (fov < 1.0f || fov > 179.0f) {
        fov = DEFAULT_FOV;
    }
    // A point one unit off the camera's axis for each unit of depth is this many pixels off the
    // screen's center: half the screen's height over the tangent of half the field of view.
    halfFov = (s16) (fov * (0x8000 / 360.0f));
    perUnit = SCREEN_CENTER_Y * coss(halfFov) / sins(halfFov);
    for (slot = 0; slot < MP_PUPPETS && drawn < NAMES_MOST; slot++) {
        if (has_room(MP_NAME_BYTES) && draw_name(slot, perUnit)) {
            drawn++;
        }
    }
}

// How many of the chat's newest rows the bridge asks for now.
static s32 chat_rows_shown(void) {
    s32 shown = gMpMailbox->chat.shown;

    if (shown > MP_CHAT_SHOWN_MOST) {
        shown = MP_CHAT_SHOWN_MOST;
    }
    if ((u32) shown > gMpMailbox->chat.count) {
        shown = gMpMailbox->chat.count;
    }
    return shown;
}

// The chat's box, drawn while the player types: a panel behind the `rows` rows shown and the
// row being typed, and the same again over that row alone, which makes it the darker, as a
// field to type in. Before the text is set up, as shading changes what that sets.
static void shade_chat(s32 rows) {
    s32 top = rows == 0 ? CHAT_STRIP_TOP : CHAT_BOTTOM + rows * LINE_HEIGHT + CHAT_PAD - 1;

    shade(CHAT_BOX_LEFT, CHAT_BOX_BOTTOM, CHAT_BOX_RIGHT, top);
    shade(CHAT_BOX_LEFT, CHAT_BOX_BOTTOM, CHAT_BOX_RIGHT, CHAT_STRIP_TOP);
}

// Draws the newest `shown` rows of the chat, the newest lowest, and under them, while the
// player types, the row being typed. A row there is no room for is left out, and those above.
static void draw_chat(s32 shown, s32 typing) {
    volatile struct MpChat *chat = &gMpMailbox->chat;
    u8 row[MP_CHAT_ROW_BYTES];
    u32 count = chat->count;
    s32 i;

    if (typing && has_room(take_text(row, chat->draft, MP_CHAT_ROW_BYTES - 1))) {
        draw_text(CHAT_LEFT, CHAT_DRAFT_BOTTOM, row);
    }
    for (i = 0; i < shown; i++) {
        if (!has_room(take_text(row, chat->rows[(count - 1 - i) % MP_CHAT_ROWS],
                                MP_CHAT_ROW_BYTES - 1))) {
            break;
        }
        draw_text(CHAT_LEFT, CHAT_BOTTOM + i * LINE_HEIGHT, row);
    }
}

// Writes a number of up to three digits at `out`, and returns where the next character goes.
static u8 *put_number(u8 *out, s32 value) {
    if (value < 0 || value > 999) {
        value = 0;
    }
    if (value >= 100) {
        *out++ = '0' + value / 100;
    }
    if (value >= 10) {
        *out++ = '0' + value / 10 % 10;
    }
    *out++ = '0' + value % 10;
    return out;
}

// Draws row `index` of the list, counted down each column in turn: a crown for the host or a
// shield for a moderator, the name, and at the column's right the level and area as "4-1".
static void draw_list_row(s32 index, s32 who, volatile u8 *name, s32 level, s32 area) {
    u8 text[MP_NAME_BYTES];
    u8 place[PLACE_BYTES];
    u8 *out = place;
    s32 role = who & MP_ROLE_MASK;
    s32 x = LIST_LEFT + index / LIST_COLUMN_ROWS * LIST_COLUMN_WIDTH;
    s32 y = LIST_TOP - index % LIST_COLUMN_ROWS * LINE_HEIGHT;

    // A row there is no room for is left out: with sixteen long names, the last ones may be.
    if (!has_room(MP_NAME_BYTES + PLACE_BYTES)) {
        return;
    }
    if (role == MP_ROLE_HOST) {
        // Gold, as a crown is, and as the game's Power Stars are.
        text_color(255, 216, 0);
        draw_cell(x, y, MARK_HOST);
    } else if (role == MP_ROLE_MODERATOR) {
        text_color(110, 200, 255);
        draw_cell(x, y, MARK_MODERATOR);
    }
    text_color(255, 255, 255);
    take_text(text, name, MP_NAME_BYTES - 1);
    draw_text(x + LIST_NAME_LEFT, y, text);
    out = put_number(out, level);
    *out++ = '-';
    out = put_number(out, area);
    *out = MP_TEXT_END;
    draw_text(x + LIST_COLUMN_WIDTH - LIST_COLUMN_GAP - text_width(place), y, place);
}

// Draws everyone in the session, the local player first, with where each is.
static void draw_list(void) {
    volatile struct MpPeople *people = &gMpMailbox->people;
    volatile struct MpPlayerSlot *player;
    s32 rows = 1;
    s32 deepest;
    s32 slot;
    s32 index = 0;

    for (slot = 0; slot < MP_PUPPETS; slot++) {
        rows += gMpMailbox->players[slot].active != 0;
    }
    // One column's worth of rows at most is as deep as the list goes.
    deepest = rows < LIST_COLUMN_ROWS ? rows : LIST_COLUMN_ROWS;
    shade(LIST_LEFT - LIST_SHADE_EDGE, LIST_TOP - (deepest - 1) * LINE_HEIGHT - LIST_SHADE_EDGE,
          SCREEN_WIDTH - LIST_LEFT + LIST_SHADE_EDGE, LIST_TOP + CELL_HEIGHT + LIST_SHADE_EDGE);
    gSPDisplayList(gDisplayListHead++, gMpTextBegin);
    draw_list_row(index, people->localRole, people->localName, gCurrLevelNum, gCurrAreaIndex);
    for (slot = 0; slot < MP_PUPPETS; slot++) {
        player = &gMpMailbox->players[slot];
        if (player->active) {
            index++;
            draw_list_row(index, people->roles[slot], people->names[slot], player->levelNum,
                          player->areaIndex);
        }
    }
}

// The game's HUD, and then the other players' part of it. See Multiplayer/people.h.
void mp_render_hud(void) {
    s32 rows;
    s32 typing;

    render_hud();
    // Nothing until a bridge has said who is here, nor once its lease has run out, when no
    // session is left to show; an odd sequence means it is writing now, and the text is as
    // good a frame late.
    if (gMpMailbox->bridgeLease == 0 || gMpMailbox->people.sequence == 0
        || (gMpMailbox->people.sequence & 1)) {
        return;
    }
    // Never while typing, when the game is shown no button as held.
    if (gPlayer1Controller->buttonDown & LIST_BUTTON) {
        draw_list();
    } else {
        rows = chat_rows_shown();
        // With no line, the keys are only being kept from Mario until those held as the last
        // one was sent are let go: nothing is drawn for that.
        typing = gMpMailbox->chat.typing != 0 && gMpMailbox->chat.draft[0] != MP_TEXT_END;
        if (typing) {
            shade_chat(rows);
        }
        gSPDisplayList(gDisplayListHead++, gMpTextBegin);
        text_color(255, 255, 255);
        draw_names();
        draw_chat(rows, typing);
    }
    gSPDisplayList(gDisplayListHead++, gMpTextEnd);
}

// The game reads its controllers through this, in place of the library's own routine: see
// where it is hooked in, Multiplayer/hooks.asm. While the player types a line of chat it
// shows the game no button held and no stick moved, whatever is: the keys they type with may
// be the ones they play with, and Mario is not to run about for a "w", nor the game to pause
// for the Enter that sends the line.
//
// The bridge sets `typing` to MP_TYPING_LEASE again and again while the line is open, and this
// counts it down each time the game reads its controllers, so a bridge that was killed leaves
// the controls dead for a second and no longer.
//
// The bridge's own lease is counted down here too, as this runs once in every frame, paused
// or not, where Mario's update does not. A bridge that was killed took nothing out of the
// game: once its lease runs out, the other players go, the chat leaves the screen, and every
// object runs as it would alone.
void mp_read_pads(OSContPad *pads) {
    osContGetReadData(pads);
    if (gMpMailbox->bridgeLease != 0 && --gMpMailbox->bridgeLease == 0) {
        bzero(gMpMailbox->players, sizeof(gMpMailbox->players));
        gMpMailbox->objectsOwned.levelNum = MP_NO_LEVEL;
        gMpMailbox->chat.shown = 0;
        // Nor is Mario dressed for a session any more. A bridge that is still there after
        // all writes who is who again within a moment.
        gMpMailbox->people.localRole = 0;
        // Nor is the session's progress added to a save any more: see read_session_progress.
        gMpMailbox->progressIn.sequence = 0;
    }
    if (gMpMailbox->chat.typing != 0) {
        gMpMailbox->chat.typing--;
        bzero(pads, MAXCONTROLLERS * sizeof(OSContPad));
    }
}
