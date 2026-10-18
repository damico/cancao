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
 * tampo.c - convolução particionada com FFT radix-2 própria.
 *
 * Truque usado: as respostas esquerda (hL) e direita (hR) são reais,
 * assim como a entrada x. Guardando H = FFT(hL + j*hR), uma única
 * convolução complexa dá y = x*hL + j*(x*hR): parte real = canal
 * esquerdo, parte imaginária = canal direito.
 */
#include "tampo.h"

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define N_FFT (2 * TAMPO_BLOCO)

struct Tampo {
    int    n_part;               /* número de partições da resposta */
    float *h_re, *h_im;          /* espectros da resposta: n_part * N_FFT */
    float *x_re, *x_im;          /* espectros das últimas entradas (anel) */
    int    pos;                  /* partição mais recente no anel */
    float  anterior[TAMPO_BLOCO];/* bloco anterior (overlap-save) */
    float  tw_re[N_FFT / 2], tw_im[N_FFT / 2];
    int    rev[N_FFT];
    float  *ir_l, *ir_r;         /* resposta no tempo, para salvar */
    int    ir_len;
};

/* ---------- FFT ---------- */

static void fft_init(Tampo *t)
{
    int bits = 0;
    while ((1 << bits) < N_FFT) bits++;
    for (int i = 0; i < N_FFT; i++) {
        int r = 0;
        for (int b = 0; b < bits; b++)
            if (i & (1 << b)) r |= 1 << (bits - 1 - b);
        t->rev[i] = r;
    }
    for (int i = 0; i < N_FFT / 2; i++) {
        t->tw_re[i] = cosf(-2.0f * (float)M_PI * i / N_FFT);
        t->tw_im[i] = sinf(-2.0f * (float)M_PI * i / N_FFT);
    }
}

/* in-place; inversa = 1 conjuga os fatores e divide por N */
static void fft(const Tampo *t, float *re, float *im, int inversa)
{
    for (int i = 0; i < N_FFT; i++) {
        int j = t->rev[i];
        if (j > i) {
            float a = re[i]; re[i] = re[j]; re[j] = a;
            a = im[i]; im[i] = im[j]; im[j] = a;
        }
    }
    float sinal = inversa ? -1.0f : 1.0f;
    for (int tam = 2; tam <= N_FFT; tam <<= 1) {
        int meio = tam / 2, passo = N_FFT / tam;
        for (int ini = 0; ini < N_FFT; ini += tam) {
            for (int k = 0; k < meio; k++) {
                float wr = t->tw_re[k * passo], wi = sinal * t->tw_im[k * passo];
                int a = ini + k, b = a + meio;
                float br = re[b] * wr - im[b] * wi;
                float bi = re[b] * wi + im[b] * wr;
                re[b] = re[a] - br; im[b] = im[a] - bi;
                re[a] += br;        im[a] += bi;
            }
        }
    }
    if (inversa)
        for (int i = 0; i < N_FFT; i++) { re[i] /= N_FFT; im[i] /= N_FFT; }
}

/* ---------- montagem ---------- */

static Tampo *tampo_de_ir(const float *ir_l, const float *ir_r, int len)
{
    Tampo *t = calloc(1, sizeof *t);
    if (!t) return NULL;
    fft_init(t);

    t->n_part = (len + TAMPO_BLOCO - 1) / TAMPO_BLOCO;
    size_t tam = (size_t)t->n_part * N_FFT;
    t->h_re = calloc(tam, sizeof(float));
    t->h_im = calloc(tam, sizeof(float));
    t->x_re = calloc(tam, sizeof(float));
    t->x_im = calloc(tam, sizeof(float));
    t->ir_l = malloc(len * sizeof(float));
    t->ir_r = malloc(len * sizeof(float));
    if (!t->h_re || !t->h_im || !t->x_re || !t->x_im || !t->ir_l || !t->ir_r) {
        tampo_liberar(t);
        return NULL;
    }
    memcpy(t->ir_l, ir_l, len * sizeof(float));
    memcpy(t->ir_r, ir_r, len * sizeof(float));
    t->ir_len = len;

    /* cada partição: TAMPO_BLOCO amostras da resposta + zeros */
    for (int p = 0; p < t->n_part; p++) {
        float *re = t->h_re + (size_t)p * N_FFT, *im = t->h_im + (size_t)p * N_FFT;
        for (int i = 0; i < TAMPO_BLOCO; i++) {
            int k = p * TAMPO_BLOCO + i;
            if (k < len) { re[i] = ir_l[k]; im[i] = ir_r[k]; }
        }
        fft(t, re, im, 0);
    }
    return t;
}

/* normaliza a energia de cada canal para 1 (volume parecido com o seco) */
static void normalizar(float *h, int len)
{
    double e = 0.0;
    for (int i = 0; i < len; i++) e += (double)h[i] * h[i];
    if (e <= 0.0) return;
    float g = (float)(1.0 / sqrt(e));
    for (int i = 0; i < len; i++) h[i] *= g;
}

static float aleatorio(unsigned *s)  /* uniforme em [0, 1) */
{
    *s = *s * 1664525u + 1013904223u;
    return (*s >> 8) * (1.0f / 16777216.0f);
}

Tampo *tampo_sintetico(float taxa, float duracao_s, unsigned semente)
{
    int len = (int)(duracao_s * taxa);
    float *h[2] = { calloc(len, sizeof(float)), calloc(len, sizeof(float)) };
    if (!h[0] || !h[1]) { free(h[0]); free(h[1]); return NULL; }

    unsigned s = semente;
    /* tampo: ~300 modos de placa de 50 Hz a 9 kHz, cada um com Q entre
     * 15 e 50; a madeira amortece muito os agudos, por isso o decaimento
     * (tau = Q / (pi f)) é curto em frequências altas. Os dois ouvidos
     * escutam os mesmos modos (é a mesma placa), com amplitude e fase
     * um pouco diferentes conforme a posição */
    for (int m = 0; m < 300; m++) {
        float f = 50.0f * powf(9000.0f / 50.0f, aleatorio(&s));
        float q = 15.0f + 35.0f * aleatorio(&s);
        float tau = q / ((float)M_PI * f);
        float fase = 2.0f * (float)M_PI * aleatorio(&s);
        /* resposta média do tampo: sobe até ~150 Hz, cai acima de ~2,5 kHz */
        float forma = (f / (f + 150.0f)) / (1.0f + (f / 2500.0f) * (f / 2500.0f));
        float amp = forma * (0.3f + aleatorio(&s)) * (aleatorio(&s) < 0.5f ? -1.0f : 1.0f);

        int n_max = (int)(tau * 7.0f * taxa);  /* até ~-60 dB */
        if (n_max > len) n_max = len;
        float w = 2.0f * (float)M_PI * f / taxa;
        float dec = expf(-1.0f / (tau * taxa));
        for (int ch = 0; ch < 2; ch++) {
            float a = amp * (0.6f + 0.8f * aleatorio(&s));
            float ph = fase + 0.8f * (aleatorio(&s) - 0.5f);
            /* oscilador recursivo: evita sinf() por amostra */
            float c = cosf(ph), sn = sinf(ph), cr = cosf(w), sr = sinf(w);
            for (int i = 0; i < n_max; i++) {
                h[ch][i] += a * sn;
                float nc = c * cr - sn * sr;
                sn = c * sr + sn * cr;
                c = nc;
                a *= dec;
            }
        }
    }

    for (int ch = 0; ch < 2; ch++) {
        /* sala: ruído com decaimento exponencial (RT60 ~1 s), começando
         * após ~8 ms (primeiras reflexões), filtrado para escurecer */
        float rt60 = 1.0f, tau_sala = rt60 / 6.91f;
        int ini = (int)(0.008f * taxa);
        float lp = 0.0f, nivel = 0.05f;
        for (int i = ini; i < len; i++) {
            float t_s = (float)(i - ini) / taxa;
            float ataque = t_s < 0.02f ? t_s / 0.02f : 1.0f;
            float r = 2.0f * aleatorio(&s) - 1.0f;
            lp += 0.35f * (r - lp);
            h[ch][i] += nivel * lp * ataque * expf(-t_s / tau_sala);
        }
        normalizar(h[ch], len);
    }

    Tampo *t = tampo_de_ir(h[0], h[1], len);
    free(h[0]);
    free(h[1]);
    return t;
}

static uint32_t ler_u32(const unsigned char *p) { return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24; }
static uint16_t ler_u16(const unsigned char *p) { return (uint16_t)(p[0] | p[1] << 8); }

Tampo *tampo_de_wav(const char *caminho, float taxa, float duracao_max_s)
{
    FILE *f = fopen(caminho, "rb");
    if (!f) { perror(caminho); return NULL; }
    fseek(f, 0, SEEK_END);
    long tam = ftell(f);
    fseek(f, 0, SEEK_SET);
    unsigned char *d = malloc(tam);
    if (!d || fread(d, 1, tam, f) != (size_t)tam) { fclose(f); free(d); return NULL; }
    fclose(f);

    Tampo *t = NULL;
    int formato = 0, canais = 0, bits = 0;
    unsigned taxa_wav = 0;
    if (tam < 12 || memcmp(d, "RIFF", 4) || memcmp(d + 8, "WAVE", 4)) {
        fprintf(stderr, "%s: não é WAV\n", caminho);
        goto fim;
    }
    for (long p = 12; p + 8 <= tam;) {
        uint32_t n = ler_u32(d + p + 4);
        const unsigned char *c = d + p + 8;
        if (!memcmp(d + p, "fmt ", 4) && n >= 16) {
            formato = ler_u16(c);
            canais = ler_u16(c + 2);
            taxa_wav = ler_u32(c + 4);
            bits = ler_u16(c + 14);
        } else if (!memcmp(d + p, "data", 4)) {
            int pcm16 = formato == 1 && bits == 16, f32 = formato == 3 && bits == 32;
            if (!(pcm16 || f32) || canais < 1 || canais > 2) {
                fprintf(stderr, "%s: use PCM 16 bits ou float 32, mono/estéreo\n", caminho);
                goto fim;
            }
            if (p + 8 + (long)n > tam) n = (uint32_t)(tam - p - 8);
            if (taxa_wav != (unsigned)taxa)
                fprintf(stderr, "aviso: %s está a %u Hz, esperado %.0f Hz (sem reamostragem)\n",
                        caminho, taxa_wav, taxa);
            int len = (int)(n / (bits / 8) / canais);
            int max = (int)(duracao_max_s * taxa);
            if (len > max) len = max;
            float *l = malloc(len * sizeof(float)), *r = malloc(len * sizeof(float));
            for (int i = 0; l && r && i < len; i++) {
                for (int ch = 0; ch < 2; ch++) {
                    int cc = ch < canais ? ch : 0;
                    const unsigned char *s = c + ((size_t)i * canais + cc) * (bits / 8);
                    float v;
                    if (pcm16) v = (int16_t)ler_u16(s) / 32768.0f;
                    else { uint32_t u = ler_u32(s); memcpy(&v, &u, 4); }
                    (ch ? r : l)[i] = v;
                }
            }
            if (l && r) {
                normalizar(l, len);
                normalizar(r, len);
                t = tampo_de_ir(l, r, len);
            }
            free(l);
            free(r);
            goto fim;
        }
        p += 8 + n + (n & 1);
    }
    fprintf(stderr, "%s: sem bloco de dados\n", caminho);
fim:
    free(d);
    return t;
}

/* ---------- processamento em tempo real ---------- */

void tampo_processar(Tampo *t, const float *entrada, float *saida)
{
    /* espectro de [bloco anterior | bloco atual] entra no anel */
    t->pos = (t->pos + 1) % t->n_part;
    float *xr = t->x_re + (size_t)t->pos * N_FFT, *xi = t->x_im + (size_t)t->pos * N_FFT;
    memcpy(xr, t->anterior, sizeof t->anterior);
    memcpy(xr + TAMPO_BLOCO, entrada, sizeof(float) * TAMPO_BLOCO);
    memset(xi, 0, sizeof(float) * N_FFT);
    memcpy(t->anterior, entrada, sizeof t->anterior);
    fft(t, xr, xi, 0);

    /* Y = soma_p X[atual - p] * H[p] */
    float yr[N_FFT] = { 0 }, yi[N_FFT] = { 0 };
    for (int p = 0; p < t->n_part; p++) {
        int q = t->pos - p;
        if (q < 0) q += t->n_part;
        const float *restrict ar = t->x_re + (size_t)q * N_FFT, *restrict ai = t->x_im + (size_t)q * N_FFT;
        const float *restrict hr = t->h_re + (size_t)p * N_FFT, *restrict hi = t->h_im + (size_t)p * N_FFT;
        for (int k = 0; k < N_FFT; k++) {
            yr[k] += ar[k] * hr[k] - ai[k] * hi[k];
            yi[k] += ar[k] * hi[k] + ai[k] * hr[k];
        }
    }
    fft(t, yr, yi, 1);

    /* overlap-save: a metade final é a saída válida */
    for (int i = 0; i < TAMPO_BLOCO; i++) {
        saida[2 * i]     = yr[TAMPO_BLOCO + i];
        saida[2 * i + 1] = yi[TAMPO_BLOCO + i];
    }
}

int tampo_salvar_ir(const Tampo *t, const char *caminho, int taxa)
{
    FILE *f = fopen(caminho, "wb");
    if (!f) { perror(caminho); return -1; }
    uint32_t n = (uint32_t)t->ir_len, bytes = n * 8;
    uint32_t cab32[] = { 36 + bytes, 16, (uint32_t)taxa, (uint32_t)taxa * 8 };
    uint16_t fmt[] = { 3, 2 }, bloco[] = { 8, 32 };
    fwrite("RIFF", 1, 4, f); fwrite(&cab32[0], 4, 1, f);
    fwrite("WAVEfmt ", 1, 8, f); fwrite(&cab32[1], 4, 1, f);
    fwrite(fmt, 2, 2, f); fwrite(&cab32[2], 4, 2, f); fwrite(bloco, 2, 2, f);
    fwrite("data", 1, 4, f); fwrite(&bytes, 4, 1, f);
    /* normaliza o pico para ouvir confortavelmente */
    float pico = 1e-9f;
    for (uint32_t i = 0; i < n; i++)
        pico = fmaxf(pico, fmaxf(fabsf(t->ir_l[i]), fabsf(t->ir_r[i])));
    for (uint32_t i = 0; i < n; i++) {
        float lr[2] = { 0.9f * t->ir_l[i] / pico, 0.9f * t->ir_r[i] / pico };
        fwrite(lr, 4, 2, f);
    }
    fclose(f);
    return 0;
}

void tampo_liberar(Tampo *t)
{
    if (!t) return;
    free(t->h_re); free(t->h_im);
    free(t->x_re); free(t->x_im);
    free(t->ir_l); free(t->ir_r);
    free(t);
}
