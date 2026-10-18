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
 * corda.c - parâmetros físicos plausíveis para as cordas de um piano
 * de cauda (~1,9 m de corda mais grave).
 *
 * O projetista escolhe L e mu (comprimento e bitola/enrolamento);
 * a tensão T é o que o afinador ajusta para chegar na nota. Por isso
 * aqui T é obtida invertendo a fórmula, e depois f é recalculada pela
 * fórmula da corda ideal; é esse f que o sintetizador usa.
 */
#include "corda.h"

#include <ctype.h>
#include <math.h>
#include <stdio.h>
#include <string.h>

#define RHO_ACO   7850.0    /* densidade do aço (kg/m^3) */
#define E_ACO     2.0e11    /* módulo de Young do aço (Pa) */

#define F_C8      4186.009
#define F_A0      27.5
#define F_QUEBRA  168.0     /* abaixo disso o comprimento deixa de crescer ~1/f */
#define F_ENROL   110.0     /* abaixo disso as cordas são enroladas */

static const char *NOMES[12] = {
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"
};

static double lerp(double a, double b, double t) { return a + (b - a) * t; }

/* posição logarítmica de f entre f0 e f1 (0 em f0, 1 em f1) */
static double pos_log(double f, double f0, double f1)
{
    return log(f / f0) / log(f1 / f0);
}

double corda_frequencia(double L, double T, double mu)
{
    return (1.0 / (2.0 * L)) * sqrt(T / mu);
}

double corda_parcial(const Corda *c, int n)
{
    return n * c->f * sqrt(1.0 + c->B * n * n);
}

static double comprimento(double f)
{
    /* agudos: L cresce quase como 1/f (C8 ~ 5 cm) */
    double L_agudo = 0.052 * pow(F_C8 / f, 0.92);
    if (f >= F_QUEBRA)
        return L_agudo;
    /* graves: o piano não comporta 5 m de corda; o comprimento cresce
     * devagar até ~1,95 m e a massa extra vem do enrolamento */
    double L_q = 0.052 * pow(F_C8 / F_QUEBRA, 0.92);
    return lerp(L_q, 1.95, pos_log(f, F_QUEBRA, F_A0));
}

void cordas_init(Corda cordas[NUM_TECLAS], double fator_tensao)
{
    for (int k = 0; k < NUM_TECLAS; k++) {
        Corda *c = &cordas[k];
        int midi = k + 21;

        c->tecla = k;
        c->midi = midi;
        snprintf(c->nome, sizeof c->nome, "%s%d", NOMES[midi % 12], midi / 12 - 1);
        c->f_alvo = 440.0 * pow(2.0, (k - 48) / 12.0);
        c->L = comprimento(c->f_alvo);

        if (c->f_alvo >= F_ENROL) {
            /* corda lisa de aço: 0,775 mm no C8 até 1,05 mm nos médios-graves */
            double t = pos_log(c->f_alvo, F_C8, F_QUEBRA);
            if (t > 1.0) t = 1.0;
            c->d_nucleo = lerp(0.775e-3, 1.05e-3, t);
            c->mu = RHO_ACO * M_PI * c->d_nucleo * c->d_nucleo / 4.0;
            c->enrolada = 0;
            /* tensão que afina a corda: T = (2 L f)^2 mu */
            c->T = pow(2.0 * c->L * c->f_alvo, 2.0) * c->mu;
        } else {
            /* corda enrolada: o fabricante escolhe a massa do cobre para
             * chegar a uma tensão de projeto (~800 N a 1600 N) */
            double t = pos_log(c->f_alvo, F_ENROL, F_A0);
            c->d_nucleo = lerp(1.1e-3, 1.5e-3, t);
            c->enrolada = 1;
            c->T = lerp(800.0, 1600.0, t);
            c->mu = c->T / pow(2.0 * c->L * c->f_alvo, 2.0);
        }

        c->T *= fator_tensao;
        c->n_cordas = k < 8 ? 1 : (c->f_alvo < F_ENROL ? 2 : 3);
        c->f = corda_frequencia(c->L, c->T, c->mu);
        /* a rigidez vem só do núcleo de aço */
        c->B = pow(M_PI, 3) * E_ACO * pow(c->d_nucleo, 4) /
               (64.0 * c->T * c->L * c->L);
    }
}

int tecla_de_nome(const char *nome)
{
    static const int base[7] = { 9, 11, 0, 2, 4, 5, 7 }; /* A B C D E F G */
    if (!nome || !*nome) return -1;

    int letra = toupper((unsigned char)nome[0]);
    if (letra < 'A' || letra > 'G') return -1;
    int semitom = base[letra - 'A'];
    const char *p = nome + 1;
    if (*p == '#') { semitom++; p++; }
    else if (*p == 'b') { semitom--; p++; }

    int oitava;
    if (sscanf(p, "%d", &oitava) != 1) return -1;
    int midi = (oitava + 1) * 12 + semitom;
    int k = midi - 21;
    return (k >= 0 && k < NUM_TECLAS) ? k : -1;
}
