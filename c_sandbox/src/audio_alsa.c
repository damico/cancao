/*
 * Copyright 2026 José Damico <jose.damico@scicrop.com>
 * SPDX-License-Identifier: Apache-2.0
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

/*
 * audio_alsa.c - backend ALSA.
 *
 * Declaramos aqui só as funções da libasound que usamos, para compilar
 * sem o pacote de desenvolvimento (libasound2-dev); linkamos direto
 * em libasound.so.2.
 */
#include "audio.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

typedef struct _snd_pcm snd_pcm_t;
typedef unsigned long snd_pcm_uframes_t;
typedef long snd_pcm_sframes_t;

enum { SND_PCM_STREAM_PLAYBACK = 0 };
enum { SND_PCM_FORMAT_S16_LE = 2 };
enum { SND_PCM_ACCESS_RW_INTERLEAVED = 3 };

int snd_pcm_open(snd_pcm_t **pcm, const char *name, int stream, int mode);
int snd_pcm_set_params(snd_pcm_t *pcm, int format, int access,
                       unsigned int channels, unsigned int rate,
                       int soft_resample, unsigned int latency);
snd_pcm_sframes_t snd_pcm_writei(snd_pcm_t *pcm, const void *buffer,
                                 snd_pcm_uframes_t size);
int snd_pcm_recover(snd_pcm_t *pcm, int err, int silent);
int snd_pcm_drop(snd_pcm_t *pcm);
int snd_pcm_close(snd_pcm_t *pcm);
const char *snd_strerror(int errnum);

#define MAX_QUADROS 1024

struct Audio {
    snd_pcm_t *pcm;
    int16_t buf[MAX_QUADROS * 2];
};

Audio *audio_abrir(const char *dispositivo, unsigned taxa, unsigned latencia_us)
{
    Audio *a = calloc(1, sizeof *a);
    if (!a) return NULL;

    int err = snd_pcm_open(&a->pcm, dispositivo, SND_PCM_STREAM_PLAYBACK, 0);
    if (err < 0) {
        fprintf(stderr, "ALSA: não abriu '%s': %s\n", dispositivo, snd_strerror(err));
        free(a);
        return NULL;
    }
    err = snd_pcm_set_params(a->pcm, SND_PCM_FORMAT_S16_LE,
                             SND_PCM_ACCESS_RW_INTERLEAVED, 2, taxa, 1, latencia_us);
    if (err < 0) {
        fprintf(stderr, "ALSA: parâmetros recusados: %s\n", snd_strerror(err));
        snd_pcm_close(a->pcm);
        free(a);
        return NULL;
    }
    return a;
}

int audio_escrever(Audio *a, const float *quadros, int n)
{
    while (n > 0) {
        int m = n > MAX_QUADROS ? MAX_QUADROS : n;
        for (int i = 0; i < 2 * m; i++)
            a->buf[i] = (int16_t)(quadros[i] * 32767.0f);

        const int16_t *p = a->buf;
        int resto = m;
        while (resto > 0) {
            snd_pcm_sframes_t w = snd_pcm_writei(a->pcm, p, resto);
            if (w < 0) {
                /* underrun (xrun): recupera e segue */
                if (snd_pcm_recover(a->pcm, (int)w, 1) < 0)
                    return -1;
                continue;
            }
            p += 2 * w;
            resto -= (int)w;
        }
        quadros += 2 * m;
        n -= m;
    }
    return 0;
}

void audio_fechar(Audio *a)
{
    if (!a) return;
    /* drop, não drain: drain espera o buffer tocar e trava para sempre
     * se o servidor de som parou de consumir */
    snd_pcm_drop(a->pcm);
    snd_pcm_close(a->pcm);
    free(a);
}
