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
 * corda.h - modelo físico das 88 cordas de um piano.
 *
 * A frequência fundamental de uma corda ideal é
 *
 *        f = (1 / 2L) * sqrt(T / mu)
 *
 * L  = comprimento vibrante (m)
 * T  = tensão (N)
 * mu = massa por unidade de comprimento (kg/m)
 *
 * Uma corda real de aço tem rigidez, o que torna os parciais
 * levemente "esticados" (inarmonicidade):
 *
 *        f_n = n * f * sqrt(1 + B n^2),   B = pi^3 E d^4 / (64 T L^2)
 */
#ifndef CORDA_H
#define CORDA_H

#define NUM_TECLAS 88   /* A0 (tecla 0) .. C8 (tecla 87) */

typedef struct {
    int    tecla;       /* 0..87 */
    int    midi;        /* 21..108 */
    char   nome[5];     /* "A0", "C#4", ... */
    double f_alvo;      /* temperamento igual, A4 = 440 Hz */
    double L;           /* comprimento vibrante (m) */
    double mu;          /* massa linear (kg/m) */
    double T;           /* tensão (N) */
    double d_nucleo;    /* diâmetro do fio de aço (m) */
    int    enrolada;    /* 1 = corda de baixo com enrolamento de cobre */
    int    n_cordas;    /* cordas por nota (1, 2 ou 3) */
    double f;           /* fundamental pela fórmula da corda ideal */
    double B;           /* coeficiente de inarmonicidade */
} Corda;

/* Monta as 88 cordas. fator_tensao multiplica a tensão de todas
 * (1.0 = afinado; 1.02 = 2% mais tensas, ~ +17 cents). */
void cordas_init(Corda cordas[NUM_TECLAS], double fator_tensao);

/* f = 1/(2L) * sqrt(T/mu) */
double corda_frequencia(double L, double T, double mu);

/* Frequência do n-ésimo parcial, com inarmonicidade. */
double corda_parcial(const Corda *c, int n);

/* Converte "A4", "C#3", "Bb2" na tecla 0..87; -1 se inválido. */
int tecla_de_nome(const char *nome);

#endif
