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
 * synth.h - sintetizador polifônico de piano por síntese modal
 * (soma de parciais inarmônicos com decaimento independente).
 */
#ifndef SYNTH_H
#define SYNTH_H

#include "corda.h"
#include "tampo.h"

#define MAX_VOZES    32
#define MAX_PARCIAIS 48
#define MAX_UNISSONO 3
#define MAX_OSC      (MAX_PARCIAIS * MAX_UNISSONO)

typedef struct {
    int   ativa;
    int   tecla;
    int   abafada;
    int   n_osc;
    unsigned idade;

    /* osciladores em estrutura-de-arrays para o compilador vetorizar */
    float c[MAX_OSC], s[MAX_OSC];     /* fasor (cos, sen) */
    float cr[MAX_OSC], sr[MAX_OSC];   /* rotação por amostra */
    float a1[MAX_OSC], a2[MAX_OSC];   /* envelope em 2 estágios */
    float d1[MAX_OSC], d2[MAX_OSC];   /* decaimento por amostra */

    float ruido_amp, ruido_dec, ruido_lp;  /* "thump" do martelo */
    unsigned rng;
    float ganho_e, ganho_d;                /* pan estéreo */
} Voz;

typedef struct {
    float taxa;
    Corda cordas[NUM_TECLAS];
    Voz   vozes[MAX_VOZES];
    unsigned relogio;
    float volume;
    Tampo *tampo;     /* NULL = sem tampo (som "seco" das cordas) */
    float mix_seco;   /* quanto do som direto das cordas misturar ao do tampo */
    float ganho_tampo;
} Synth;

void synth_init(Synth *s, float taxa, double fator_tensao);

/* velocidade 0..1 */
void synth_nota_on(Synth *s, int tecla, float velocidade);
void synth_nota_off(Synth *s, int tecla);
void synth_abafar_tudo(Synth *s);

/* Gera n quadros estéreo intercalados (float, -1..1).
 * Com tampo, n precisa ser múltiplo de TAMPO_BLOCO. */
void synth_render(Synth *s, float *saida, int n);

int synth_vozes_ativas(const Synth *s);

#endif
