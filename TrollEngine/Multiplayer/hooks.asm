; Points the game's calls at the multiplayer code; included by Multiplayer/overlay.asm.
;
; The game's own code is not built again, so each hook changes one instruction of it, and first
; checks that the ROM holds the instruction it expects there: a version file that does not fit
; the ROM stops the build instead of making a ROM that crashes. Names that begin `mp_site_`, and
; the engine functions named here, come from the version's file in Multiplayer/versions/.

; The four ranges of game code hooked here, and what must be taken from an address in each to
; get its place in the ROM: the game's main segment, the vanilla engine segment, and the two
; segments Troll Engine adds (AI/ai segment.asm).
.macro mp_at, address
    .if (address) >= 0x807E0000
        .headersize 0x807E0000 - 0x188440
    .elseif (address) >= 0x80378800
        .headersize 0x80378800 - 0xF5580
    .elseif (address) >= 0x80367460
        .headersize 0x80367460 - 0x10C280
    .else
        .headersize 0x80245000
    .endif
    .org address
.endmacro

; The ROM's word at the place being assembled. armips reads a ROM's words little-endian.
MP_WORD_HERE equ ((readu8(BASE, orga()) << 24) | (readu8(BASE, orga() + 1) << 16) | (readu8(BASE, orga() + 2) << 8) | readu8(BASE, orga() + 3))

; Makes the call to `original` at `address` a call to `replacement`.
.macro mp_call, address, original, replacement
    mp_at address
    .if MP_WORD_HERE != (0x0C000000 | (((original) & 0x0FFFFFFF) >> 2))
        .error "The ROM does not call " + tohex(original) + " at " + tohex(address) + ": the version file does not fit it"
    .endif
    JAL replacement
.endmacro

; Makes the word `original` at ROM offset `offset` the address of `replacement`.
.macro mp_pointer, offset, original, replacement
    .orga offset
    .if MP_WORD_HERE != (original)
        .error "The ROM does not name " + tohex(original) + " at " + tohex(offset) + ": the version file does not fit it"
    .endif
    .word replacement
.endmacro

; Boot: setup_game_memory calls mp_setup (Multiplayer/boot.c) where it called troll_setup. It
; runs before the multiplayer segments are loaded, as it is what loads them, so it sits in the
; unused end of the engine's second segment, which the game has just loaded.
mp_call 0x80248A84, troll_setup, mp_setup
mp_at mp_boot_addr
.area 0x100, 0x01
    .if MP_WORD_HERE != 0x01010101
        .error "The ROM has no room at " + tohex(mp_boot_addr) + " for the loader: the version file does not fit it"
    .endif
    .importobj "Multiplayer/boot.o"
.endarea

; World generation (Multiplayer/world.c): each step that personalizes a level runs with the
; session's values in place of the player's, so that every player generates the same level.
mp_call 0x8027AF30, postObjectLoadPass, mp_post_object_load_pass             ; load_area
mp_call 0x8037EE68, level_init_intercept, mp_level_init_intercept            ; level_cmd_init_level
mp_call 0x8037EF78, load_stuff_in_level_pool, mp_load_stuff_in_level_pool    ; level_cmd_free_level_pool
mp_call 0x8037F02C, area_geo_load_intercept, mp_area_geo_load_intercept      ; level_cmd_begin_area
mp_call 0x8029D12C, addMoreObjects, mp_addMoreObjects                        ; spawn_objects_from_info
mp_call mp_site_bg_seg_load, interceptBgSegLoad, mp_intercept_bg_seg_load    ; interceptSegLoad
mp_call mp_site_geo_total_stars, save_file_get_total_star_count, mp_geo_total_stars ; troll_geo_layout

; get_red_star_count gives the session's count while a level generates: the whole function is
; replaced by mp_get_red_star_count. The check is of its first instructions, which make room on
; the stack and keep the return address, and its first call.
mp_at get_red_star_count
.if MP_WORD_HERE != 0x27BDFFE0
    .error "get_red_star_count does not begin as expected: the version file does not fit the ROM"
.endif
J       mp_get_red_star_count
NOP
mp_call get_red_star_count + 0x10, save_file_masked_get_course_star_count, save_file_masked_get_course_star_count

; updateRTC reads the session's clock while a session has one: it begins with a jump to
; mp_update_rtc_entry (Multiplayer/segment.asm), which either returns for it or runs the two
; instructions this jump replaced and goes on with the rest. Those two are kept here first.
; They must be ones that work anywhere: of the kinds that begin the function in the versions
; found so far, which set a register or the stack and jump nowhere.
mp_at updateRTC
.definelabel mp_update_rtc_first, MP_WORD_HERE
.skip 4
.definelabel mp_update_rtc_second, MP_WORD_HERE
.if (mp_update_rtc_first >> 26) < 8 || ((mp_update_rtc_first >> 26) >= 0x10 && (mp_update_rtc_first >> 26) < 0x18)
    .error "updateRTC begins with an instruction that cannot be moved"
.endif
.if (mp_update_rtc_second >> 26) < 8 || ((mp_update_rtc_second >> 26) >= 0x10 && (mp_update_rtc_second >> 26) < 0x18)
    .error "updateRTC's second instruction cannot be moved"
.endif
mp_at updateRTC
J       mp_update_rtc_entry
NOP

; The HUD is followed by the other players' part of it (Multiplayer/people.c); this is the call
; to render_hud in render_game.
mp_call 0x8027B454, render_hud, mp_render_hud

; Every object's update goes through multiplayer code, which mirrors objects another player's
; game runs instead of running them (Multiplayer/objects.c). These are the two calls to
; cur_obj_update, in update_objects_starting_at and update_objects_during_time_stop.
mp_call 0x8029CB78, cur_obj_update, mp_cur_obj_update
mp_call 0x8029CCB8, cur_obj_update, mp_cur_obj_update

; The game reads its controllers through multiplayer code, which shows it none of what they
; hold while the player types a line of chat (Multiplayer/people.c). This is the call to
; osContGetReadData in read_controller_inputs, the only one in the ROM.
mp_call 0x8024866C, osContGetReadData, mp_read_pads

; bhvMario's second CALL_NATIVE, bhv_mario_update, goes through multiplayer code.
; Behavior scripts run from rmseg13: the copy at their vanilla ROM offset is never loaded.
mp_pointer rmseg13 + 0x2EC0 + 0x28, bhv_mario_update, mp_mario_update

; Mario's model calls the game's own functions for how see-through he is and which body to draw.
; These are every place its layouts name them, B3313's copies included; they call through
; Multiplayer/puppet.c, which fades other players in and out.
mp_pointer 0x12A7A8, geo_mirror_mario_set_alpha, mp_mario_set_alpha
mp_pointer 0x11A46B8, geo_mirror_mario_set_alpha, mp_mario_set_alpha
mp_pointer 0x11D9A30, geo_mirror_mario_set_alpha, mp_mario_set_alpha
mp_pointer 0x12A694, geo_switch_mario_cap_effect, mp_mario_cap_effect
mp_pointer 0x12A6C8, geo_switch_mario_cap_effect, mp_mario_cap_effect
mp_pointer 0x12A6FC, geo_switch_mario_cap_effect, mp_mario_cap_effect
mp_pointer 0x11A46D4, geo_switch_mario_cap_effect, mp_mario_cap_effect
mp_pointer 0x11D9A4C, geo_switch_mario_cap_effect, mp_mario_cap_effect
