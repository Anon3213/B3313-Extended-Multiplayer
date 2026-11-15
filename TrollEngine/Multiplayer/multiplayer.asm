; Included inside the aiseg1 .area, only when building with MULTIPLAYER=1. Only the boot loader is
; here: the rest of the multiplayer code is in its own segment, Multiplayer/segment.asm.

; osInvalICache, from libultra; the address is the one Objects/blaarg/labels.asm gives.
.definelabel osInvalICache, 0x80324610

.importobj "Multiplayer/boot.o"
