MODEL = promotion_lookup_race.pml
PAN   = pan
CC    = cc
CFLAGS = -O2
DEPTH = 200000

.PHONY: all safety liveness clean

all: safety

safety: $(PAN)
	./$(PAN) -m$(DEPTH)

liveness: $(PAN)-live
	./$(PAN)-live -a -m$(DEPTH)

$(PAN): pan.c
	$(CC) $(CFLAGS) -DSAFETY -o $@ $<

$(PAN)-live: pan.c
	$(CC) $(CFLAGS) -o $@ $<

pan.c: $(MODEL)
	spin -a $<

clean:
	rm -f $(PAN) $(PAN)-live pan.* *.trail
