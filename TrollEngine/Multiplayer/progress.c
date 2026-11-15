#include "sm64.h"
#include "area.h"
#include "buffers/buffers.h"
#include "course_table.h"
#include "interaction.h"
#include "level_update.h"
#include "model_ids.h"
#include "object_fields.h"
#include "object_list_processor.h"
#include "save_file.h"

#include "Multiplayer/mailbox.h"
#include "Multiplayer/progress.h"

// Players share their progress: a star, key, cap switch or door one of them gets, every other
// player's game adds to its own save file, which it saves as usual. Each bridge reports only what
// its game got during the session, never the rest of its save, and the server adds it up; this
// game adds the session's progress to its file and saves.

extern u16 betaKeyFlags;
// B3313's own (Trolls/global_star_ids): whether the save has the star a star object's parameters
// are for. It may also mark the current object, so that must be the star.
extern u32 blue_star_check(s32 behParams);

// The save flags that are progress: the caps, keys, doors, DDD, the moat, the 50-star door and the
// castle's secret stars. The rest belong to the player, such as where their lost cap is, or are not
// known to be progress.
#define MP_SHARED_SAVE_FLAGS 0x1F10FFFE

// The star bytes, courseStars through extraStars, run straight on to keyFlags with no gap, and are
// exactly as many as the mailbox carries.
#define SAVE_FILE_OFFSET(field) ((u32) &((struct SaveFile *) 0)->field)
typedef char MpStarBytesCheck[SAVE_FILE_OFFSET(keyFlags) - SAVE_FILE_OFFSET(courseStars)
                                      == MP_STAR_BYTES
                                  ? 1
                                  : -1];

static struct SaveFile *current_file(void) {
    if (gCurrSaveFileNum < 1 || gCurrSaveFileNum > NUM_SAVE_FILES) {
        return NULL;
    }
    return &gSaveBuffer.files[gCurrSaveFileNum - 1];
}

// courseStars, courseCoinScores and extraStars follow one another, and B3313 reads them as one run
// of star flags; MpStarBytesCheck checks the run's length.
static u8 *star_bytes(struct SaveFile *file) {
    return file->courseStars;
}

void mp_publish_progress(void) {
    volatile struct MpProgress *out = &gMpMailbox->progressOut;
    struct SaveFile *file = current_file();
    u8 *stars;
    s32 i;

    out->sequence = out->sequence | 1;
    if (file == NULL) {
        out->saveFile = 0;
    } else {
        stars = star_bytes(file);
        out->flags = file->flags & MP_SHARED_SAVE_FLAGS;
        for (i = 0; i < MP_STAR_BYTES; i++) {
            out->stars[i] = stars[i];
        }
        out->keyFlags = file->keyFlags;
        out->betaKeys = betaKeyFlags;
        out->saveFile = gCurrSaveFileNum;
    }
    out->sequence = out->sequence + 1;
}

// The bridge's last consistent copy of the session's progress, if it has written one.
static s32 read_session_progress(struct MpProgress *copy) {
    volatile struct MpProgress *in = &gMpMailbox->progressIn;
    u32 sequence = in->sequence;
    s32 i;

    if (sequence == 0 || (sequence & 1)) {
        return FALSE;
    }
    copy->flags = in->flags;
    for (i = 0; i < MP_STAR_BYTES; i++) {
        copy->stars[i] = in->stars[i];
    }
    copy->keyFlags = in->keyFlags;
    copy->betaKeys = in->betaKeys;
    return in->sequence == sequence;
}

// Gives each star here that the save now has the look of a collected one, as a star gets
// when a level loads with it already collected: see-through, and still there to collect. A star
// another player collected would otherwise look uncollected until this player left the level.
static void show_collected_stars(void) {
    struct Object *current = gCurrentObject;
    struct Object *star;
    s32 slot;

    if (gLoadedGraphNodes[MODEL_STAR] == NULL || gLoadedGraphNodes[MODEL_TRANSPARENT_STAR] == NULL) {
        return;
    }
    for (slot = 0; slot < OBJECT_POOL_CAPACITY; slot++) {
        star = &gObjectPool[slot];
        if (star->activeFlags != ACTIVE_FLAG_DEACTIVATED
            && (star->oInteractType & INTERACT_STAR_OR_KEY)
            && star->header.gfx.sharedChild == gLoadedGraphNodes[MODEL_STAR]) {
            gCurrentObject = star;
            if (blue_star_check(star->oBehParams)) {
                star->header.gfx.sharedChild = gLoadedGraphNodes[MODEL_TRANSPARENT_STAR];
            }
        }
    }
    gCurrentObject = current;
}

void mp_take_progress(void) {
    struct SaveFile *file = current_file();
    struct MpProgress session;
    u8 *stars;
    u32 flags;
    s32 gained = FALSE;
    s32 i;

    if (file == NULL || !read_session_progress(&session)) {
        return;
    }
    // Only progress: whatever else the bridge wrote, nothing but these bits is ever set.
    flags = session.flags & MP_SHARED_SAVE_FLAGS;
    if ((file->flags | flags) != file->flags) {
        file->flags |= flags | SAVE_FLAG_FILE_EXISTS;
        gained = TRUE;
    }
    stars = star_bytes(file);
    for (i = 0; i < MP_STAR_BYTES; i++) {
        if ((stars[i] | session.stars[i]) != stars[i]) {
            stars[i] |= session.stars[i];
            gained = TRUE;
        }
    }
    if ((file->keyFlags | session.keyFlags) != file->keyFlags) {
        file->keyFlags |= session.keyFlags;
        gained = TRUE;
    }
    betaKeyFlags |= session.betaKeys;
    if (gained) {
        gSaveFileModified = TRUE;
        save_file_do_save(gCurrSaveFileNum - 1);
        // Mario's star counter is a count the game takes from the file only when he collects a
        // star himself, so without this it would not show a star another player got.
        gMarioState->numStars =
            save_file_get_total_star_count(gCurrSaveFileNum - 1, COURSE_MIN - 1, COURSE_MAX - 1);
        show_collected_stars();
    }
}
