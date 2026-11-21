; Adds the multiplayer code to a B3313 ROM. Multiplayer/Makefile runs armips on this file once
; for each version of the game, giving it:
;   BASE     the game's ROM, which is only read
;   OUT      the multiplayer ROM to write
;   VERSION  that game's file in Multiplayer/versions/
;
; Nothing of the engine is built again. Most versions of B3313 were built from engine sources
; nobody published, so the game's own code stays as it is in the ROM, and this only adds the
; multiplayer code and points the game's calls at it (Multiplayer/hooks.asm). Where the engine
; keeps what the multiplayer code uses differs from version to version: each version's file
; names those addresses, as scripts/find_engine_symbols.py read them out of its ROM.

.n64
.open BASE, OUT, 0

; trollfinity.asm, the engine's names for the game's own addresses, reads the ROM as BEE.
BEE equ BASE
.include "trollfinity.asm"
.include VERSION

.include "Multiplayer/segment.asm"
.include "Multiplayer/hooks.asm"
.include "Multiplayer/mario_colors.asm"

.close
