MODEL = check_pin_eviction_race.pml
CC    = cc
CFLAGS = -O2
DEPTH = 200000

.PHONY: all safety buggy liveness clean

# Default: verify the FIXED dispatcher. Expected: 0 errors, 0 unreached states.
all: safety

# The BUGGY_DROP_FALLBACK switch is a Promela (cpp) directive, so it must be
# passed to `spin -a` (which runs the model through the C preprocessor), NOT to
# cc. Each target regenerates pan.c because the two builds differ.

safety:
	spin -a $(MODEL)
	$(CC) $(CFLAGS) -DSAFETY -o pan pan.c
	./pan -m$(DEPTH)

# Model the PRE-FIX dispatcher (restores the dm.remove drop fallback).
# Expected: Spin reports an assertion violation (R-RESOLVE) and writes a
# .trail. Replay it with:
#   spin -t -p -g -l -DBUGGY_DROP_FALLBACK $(MODEL)
buggy:
	spin -DBUGGY_DROP_FALLBACK -a $(MODEL)
	$(CC) $(CFLAGS) -DSAFETY -o pan-buggy pan.c
	./pan-buggy -m$(DEPTH)

liveness:
	spin -a $(MODEL)
	$(CC) $(CFLAGS) -o pan-live pan.c
	./pan-live -a -m$(DEPTH)

clean:
	rm -f pan pan-buggy pan-live pan.* *.trail
