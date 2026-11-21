; Lets each player's Mario have colors of their own; included by Multiplayer/overlay.asm.
;
; B3313's Mario models (three, in the file loaded as segment 4 from ROM 0x11A35B8: one with a
; red cap and shirt, two with green) color each part by loading a light just before drawing it:
; a pair of commands, 03860010 <light> then 03880010 <ambient>, naming a `Lights1` in segment 4.
; Every Mario drawn shares that memory, so all would be one color. These change the pairs for
; the overalls, cap and shirt to name segment 0x0D instead, which no display list in the ROM
; uses and which with_player_colors (Multiplayer/puppet.c) points at a small table of lights
; before each Mario is drawn: overalls at 0x00, a red model's cap and shirt at 0x18, a green
; model's at 0x30. The other parts (skin, hair, gloves, shoes) keep the model's own.
;
; The pairs were found by scanning the ROM for those commands and reading each light's color.
; Each is patched at its two addresses: the light's at +4, the ambient's at +12.

; Overalls: 0000FF in the ROM.
.orga 0x11B6FF0 + 4 :: .word 0x0D000008
.orga 0x11B6FF0 + 12 :: .word 0x0D000000
.orga 0x11B7090 + 4 :: .word 0x0D000008
.orga 0x11B7090 + 12 :: .word 0x0D000000
.orga 0x11EC420 + 4 :: .word 0x0D000008
.orga 0x11EC420 + 12 :: .word 0x0D000000
.orga 0x11EC4C0 + 4 :: .word 0x0D000008
.orga 0x11EC4C0 + 12 :: .word 0x0D000000
.orga 0x11F7870 + 4 :: .word 0x0D000008
.orga 0x11F7870 + 12 :: .word 0x0D000000
; Cap and shirt of the red model: FF0000.
.orga 0x11B70F0 + 4 :: .word 0x0D000020
.orga 0x11B70F0 + 12 :: .word 0x0D000018
.orga 0x11B7128 + 4 :: .word 0x0D000020
.orga 0x11B7128 + 12 :: .word 0x0D000018
; Cap and shirt of the green models: 00FF00.
.orga 0x11EC518 + 4 :: .word 0x0D000038
.orga 0x11EC518 + 12 :: .word 0x0D000030
.orga 0x11EC5C0 + 4 :: .word 0x0D000038
.orga 0x11EC5C0 + 12 :: .word 0x0D000030
.orga 0x11F79F8 + 4 :: .word 0x0D000038
.orga 0x11F79F8 + 12 :: .word 0x0D000030
