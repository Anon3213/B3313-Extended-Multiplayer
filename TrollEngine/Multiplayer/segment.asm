; The multiplayer code, only when building with MULTIPLAYER=1. The engine's own segments are
; full, and so is the base ROM, so it goes just past the base ROM's end, in two segments one
; after the other there. mp_setup (Multiplayer/boot.c) loads both at boot into RAM the survey
; found unused: the first, the code, between the mailbox and GhostPeach.asm's flag at
; 0x807D0000; and the second, the font for names and chat, which the first has no room for,
; after the object numbers, where the puppets' animation cache began.
;
; Keep these in step with MP_CODE_ADDR, MP_CODE_END, MP_CODE2_ADDR and MP_CODE2_END in
; Multiplayer/mailbox.h.

.definelabel mpseg_start, 0x807C5C00
.definelabel mpseg_end, 0x807CC000
.definelabel mpseg2_start, 0x807D1600
.definelabel mpseg2_end, 0x807D2700

; vanilla's, which runs one object's behavior script; both of the object update loops call it.
.definelabel cur_obj_update, 0x80385C00
.definelabel mpseg_romstart, 0x3000000
.definelabel mpseg_romend, mpseg_romstart + (mpseg_end - mpseg_start)
.definelabel mpseg2_romstart, mpseg_romend
.definelabel mpseg2_romend, mpseg2_romstart + (mpseg2_end - mpseg2_start)

.definelabel bhv_mario_update, 0x8029CA58
; The two places Mario's model reads the state that makes him see-through; found by their
; addresses in the model's layouts, as trollfinity.asm lacks them.
.definelabel geo_mirror_mario_set_alpha, 0x802770A4
.definelabel geo_switch_mario_cap_effect, 0x802776D8
; Checks the free object list first; verified by disassembly, as trollfinity.asm lacks it.
.definelabel try_to_spawn_object, 0x8029EE24

.headersize mpseg_start - mpseg_romstart
.org mpseg_start
.area mpseg_end - mpseg_start, 0x00
    .importobj "Multiplayer/puppet.o"
    .importobj "Multiplayer/world.o"
    .importobj "Multiplayer/objects.o"
    .importobj "Multiplayer/progress.o"
    .importobj "Multiplayer/people.o"

    .align 4
    bhvMpPuppet:
    .word 0x00080000 ; BEGIN(OBJ_LIST_DEFAULT)
    .word 0x11010080 ; OR_INT(oFlags, OBJ_FLAG_ACTIVE_FROM_AFAR)
    .word 0x08000000 ; BEGIN_LOOP
    .word 0x0C000000, mp_puppet_update ; CALL_NATIVE
    .word 0x09000000 ; END_LOOP
.endarea

; The second segment: the font names and chat are drawn in (Multiplayer/font.txt, packed into
; font.bin by scripts/make_font.py), then the display lists text is drawn between
; (Multiplayer/font.c). The font is first, so that it sits at MP_FONT_ADDR.
.headersize mpseg2_start - mpseg2_romstart
.org mpseg2_start
.area mpseg2_end - mpseg2_start, 0x00
    .incbin "Multiplayer/font.bin"
    .importobj "Multiplayer/font.o"
.endarea

.include "Multiplayer/mario_colors.asm"
