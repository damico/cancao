CC      ?= gcc
CFLAGS  ?= -O2 -ffast-math -march=native -Wall -Wextra -std=c11 -D_DEFAULT_SOURCE
LDLIBS  = -l:libasound.so.2 -lpthread -lm

SRC = src/main.c src/corda.c src/synth.c src/tampo.c src/audio_alsa.c
OBJ = $(SRC:.c=.o)

piano: $(OBJ)
	$(CC) $(CFLAGS) -o $@ $(OBJ) $(LDLIBS)

src/%.o: src/%.c src/*.h
	$(CC) $(CFLAGS) -c -o $@ $<

clean:
	rm -f piano $(OBJ)

.PHONY: clean
