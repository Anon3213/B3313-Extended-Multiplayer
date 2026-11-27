; The multiplayer code; included by Multiplayer/overlay.asm. The engine's own segments are full,
; and so is the ROM, so it goes just past the ROM's end, in two segments one after the other
; there. mp_setup (Multiplayer/boot.c) loads both at boot into RAM the survey found unused: the
; first, the code, between the mailbox and GhostPeach.asm's flag at 0x807D0000; and the second,
; the font for names and chat, which the first has no room for, after the object numbers, where
; the puppets' animation cache began.
;
; Keep these in step with MP_CODE_ADDR, MP_CODE_END, MP_CODE2_ADDR and MP_CODE2_END in
; Multiplayer/mailbox.h.

.definelabel mpseg_start, 0x807C56C0
.definelabel mpseg_end, 0x807CC000
.definelabel mpseg2_start, 0x807D1600
.definelabel mpseg2_end, 0x807D2700

.definelabel mpseg_romstart, filesize(BASE)
.definelabel mpseg_romend, mpseg_romstart + (mpseg_end - mpseg_start)
.definelabel mpseg2_romstart, mpseg_romend
.definelabel mpseg2_romend, mpseg2_romstart + (mpseg2_end - mpseg2_start)

; The game's own, which the engine's trollfinity.asm lacks.
; Runs one object's behavior script; both of the object update loops call it.
.definelabel cur_obj_update, 0x80385C00
.definelabel bhv_mario_update, 0x8029CA58
; The two places Mario's model reads the state that makes him see-through; found by their
; addresses in the model's layouts.
.definelabel geo_mirror_mario_set_alpha, 0x802770A4
.definelabel geo_switch_mario_cap_effect, 0x802776D8
; Where the model draws what Mario holds; found the same way, at all 40 of its held-object nodes.
.definelabel geo_switch_mario_hand_grab_pos, 0x8027795C
; Checks the free object list first; verified by disassembly.
.definelabel try_to_spawn_object, 0x8029EE24
; From libultra; the address is the one the engine's Objects/blaarg/labels.asm gives.
.definelabel osInvalICache, 0x80324610

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

    ; Where updateRTC begins by jumping (Multiplayer/hooks.asm). mp_update_rtc
    ; (Multiplayer/world.c) says whether it set the clock from the session's; if not, this runs
    ; the two instructions the jump replaced and goes on with the rest of updateRTC.
    mp_update_rtc_entry:
    ADDIU   SP, SP, -0x18
    SW      RA, 0x14 (SP)
    JAL     mp_update_rtc
    NOP
    LW      RA, 0x14 (SP)
    BEQZ    V0, @@own_clock
    ADDIU   SP, SP, 0x18
    JR      RA
    NOP
    @@own_clock:
    .word mp_update_rtc_first
    .word mp_update_rtc_second
    J       updateRTC + 8
    NOP
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
