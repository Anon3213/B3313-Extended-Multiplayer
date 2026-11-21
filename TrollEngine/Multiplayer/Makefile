# Builds a multiplayer ROM from a B3313 ROM. Run from TrollEngine/, after `make tools` there:
#
#   make -f Multiplayer/Makefile VERSION=a2ae BASE=<the game's ROM> OUT=<the ROM to write>
#
# VERSION names a file in Multiplayer/versions/. The engine is not built: its compiler,
# assembler and headers are used, and Multiplayer/overlay.asm says what is done to the ROM.

ifndef VERSION
$(error Give VERSION, BASE and OUT; see the top of Multiplayer/Makefile)
endif

CC = tools/ido-static-recomp/build/out/cc
ARMIPS = tools/armips
N64CKSUM = tools/n64cksum

# The engine's own flags (its Makefile's CFLAGS), so that this code is compiled as the game's is.
INCLUDE_DIRS := DecompHeaders/include DecompHeaders/src DecompHeaders/. . DecompHeaders/include/libc DecompHeaders/src/game
CFLAGS := -c -G 0 -O2 -Wo,-loopunroll,0 -nostdinc -DTARGET_N64 -D_LANGUAGE_C -mips2 \
	$(foreach dir,$(INCLUDE_DIRS),-I$(dir)) -DVERSION_US=1 -DF3D_OLD=1 \
	-non_shared -Wab,-r4300_mul -Xcpluscomm -Xfullwarn -signed -32

SRC := $(wildcard Multiplayer/*.c)
OBJ := $(SRC:.c=.o)
ASM := $(wildcard Multiplayer/*.asm) Multiplayer/versions/$(VERSION).asm

# What armips writes as each symbol's address, beside the ROM: the scripts read it.
SYM = $(basename $(OUT)).sym

$(OUT): $(OBJ) $(ASM) Multiplayer/font.bin $(BASE)
	$(ARMIPS) Multiplayer/overlay.asm -strequ BASE "$(BASE)" -strequ OUT "$(OUT)" \
		-strequ VERSION Multiplayer/versions/$(VERSION).asm -sym "$(SYM)"
	$(N64CKSUM) "$(OUT)"

# Every object is built again when any header here changes: a stale one once shipped a segment
# loaded at its old address.
#
# The sources' checks of sizes and places are arrays of a negative size when they fail, which
# this compiler only warns of, as warning 654, and compiles all the same: that warning stops
# the build here, and takes the object with it.
Multiplayer/%.o: Multiplayer/%.c $(wildcard Multiplayer/*.h)
	@echo $(CC) -o $@ $<
	@$(CC) $(CFLAGS) -o $@ $< > $@.log 2>&1; status=$$?; cat $@.log; \
	if [ $$status -ne 0 ] || grep -q "Warning 654" $@.log; then \
		rm -f $@ $@.log; echo "$<: not built"; exit 1; \
	fi; rm -f $@.log

clean:
	rm -f $(OBJ)

.PHONY: clean
