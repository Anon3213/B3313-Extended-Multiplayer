#include <PR/ultratypes.h>
#include <PR/os_cache.h>

#include "Multiplayer/mailbox.h"

// The rest of the multiplayer code lives in its own segment, Multiplayer/segment.asm: the engine's
// own segments have no room left. It sits past the end of the base ROM and loads into RAM the
// survey found unused, before anything can call into it.

extern void troll_setup(void);
extern void dma_read(u8 *dest, u8 *srcStart, u8 *srcEnd);
extern u8 mpseg_romstart[];
extern u8 mpseg_romend[];
extern u8 mpseg2_romstart[];
extern u8 mpseg2_romend[];

// Runs once at boot instead of troll_setup; see AI/ai segment.asm. Marking the mailbox this early
// lets the bridge deliver the session's world before the first level generates.
void mp_setup(void) {
    dma_read((u8 *) MP_CODE_ADDR, mpseg_romstart, mpseg_romend);
    // The DMA wrote around the CPU's caches, and this range now holds instructions.
    osInvalICache((void *) MP_CODE_ADDR, MP_CODE_END - MP_CODE_ADDR);
    // The second segment, which follows the first in the ROM. It holds no instructions, only
    // the font and its display lists; code put there would need its range cleared as above.
    dma_read((u8 *) MP_CODE2_ADDR, mpseg2_romstart, mpseg2_romend);
    troll_setup();
    gMpMailbox->magic = MP_MAILBOX_MAGIC;
    gMpMailbox->version = MP_MAILBOX_VERSION;
}
