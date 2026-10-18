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
 * synth.c - cada nota é um conjunto de osciladores senoidais, um por
 * parcial e por corda do uníssono. A física entra em:
 *   - frequências: f_n = n f sqrt(1 + B n^2), f vindo de f = 1/(2L) sqrt(T/mu)
 *   - amplitudes:  martelo batendo a 1/8 do comprimento -> sin(n pi / 8)
 *                  e feltro do martelo agindo como passa-baixa (mais forte = mais brilho)
 *   - decaimento:  graves soam por mais tempo; parciais agudos morrem antes;
 *                  dois estágios (som "imediato" + "pós-som")
 *   - uníssono:    2-3 cordas por nota com diferenças mínimas de tensão,
 *                  o que gera o batimento lento típico do piano
 */
#include "synth.h"

#include <math.h>
#include <string.h>

#define POS_MARTELO   0.12f    /* ponto de batida / L */
#define TECLA_SEM_ABAFADOR 70  /* do F#6 para cima não há abafadores */
#define TAU_ABAFADO   0.08f    /* s */
#define BLOCO         TAMPO_BLOCO

static float clampf(float x, float lo, float hi)
{
    return x < lo ? lo : (x > hi ? hi : x);
}

void synth_init(Synth *s, float taxa, double fator_tensao)
{
    memset(s, 0, sizeof *s);
    s->taxa = taxa;
    s->volume = 0.8f;
    s->mix_seco = 0.2f;
    s->ganho_tampo = 0.18f;
    cordas_init(s->cordas, fator_tensao);
}

static Voz *achar_voz(Synth *s, int tecla)
{
    Voz *livre = NULL, *mais_velha = &s->vozes[0];
    for (int i = 0; i < MAX_VOZES; i++) {
        Voz *v = &s->vozes[i];
        if (v->ativa && v->tecla == tecla)
            return v;  /* re-ataque na mesma corda */
        if (!v->ativa && !livre)
            livre = v;
        if (v->idade < mais_velha->idade)
            mais_velha = v;
    }
    return livre ? livre : mais_velha;
}

void synth_nota_on(Synth *s, int tecla, float vel)
{
    if (tecla < 0 || tecla >= NUM_TECLAS) return;
    vel = clampf(vel, 0.05f, 1.0f);

    const Corda *c = &s->cordas[tecla];
    Voz *v = achar_voz(s, tecla);
    memset(v, 0, sizeof *v);
    v->ativa = 1;
    v->tecla = tecla;
    v->idade = ++s->relogio;
    v->rng = 0x9E3779B9u ^ (unsigned)(tecla * 7919 + s->relogio);

    float taxa = s->taxa;
    float f_max = fminf(0.45f * taxa, 12000.0f);

    /* feltro: corte do passa-baixa sobe com a velocidade */
    float fc = fmaxf((float)c->f * (2.0f + 10.0f * vel * vel),
                     300.0f + 3500.0f * vel * vel);

    /* decaimento base: ~4 s no A0, ~0,8 s no A4, ~0,2 s no C8 (1º estágio);
     * o 2º estágio é 6x mais longo */
    float tau1 = 4.0f * powf((float)c->f / 27.5f, -0.6f);
    float tau2 = tau1 * 6.0f;

    /* diferenças de afinação entre as cordas do uníssono, em cents;
     * vêm de diferenças de tensão: dT/T = 2 df/f */
    static const float desafino[MAX_UNISSONO] = { 0.0f, 0.9f, -0.6f };
    int nu = c->n_cordas;

    float amps[MAX_PARCIAIS];
    float fns[MAX_PARCIAIS];
    int np = 0;
    double soma2 = 0.0;
    for (int n = 1; n <= MAX_PARCIAIS; n++) {
        float fn = (float)corda_parcial(c, n);
        if (fn >= f_max) break;
        float a = fabsf(sinf(n * (float)M_PI * POS_MARTELO)) / powf((float)n, 0.7f);
        a /= 1.0f + (fn / fc) * (fn / fc);
        amps[np] = a;
        fns[np] = fn;
        soma2 += a * a;
        np++;
    }
    float norm = soma2 > 0 ? 1.0f / sqrtf((float)soma2) / nu : 0.0f;
    /* compensa a perda de volume percebido nos agudos curtos */
    float brilho = c->f > 440.0 ? powf((float)c->f / 440.0f, 0.4f) : 1.0f;
    float ganho = 0.35f * powf(vel, 1.6f) * norm * brilho;

    int i = 0;
    for (int p = 0; p < np; p++) {
        /* parciais acima da fundamental morrem antes */
        float r = (fns[p] - fns[0]) / 3000.0f;
        float tau_p = 1.0f / (1.0f + r * r);
        float m1 = expf(-1.0f / (tau1 * tau_p * taxa));
        float m2 = expf(-1.0f / (tau2 * tau_p * taxa));
        for (int u = 0; u < nu; u++, i++) {
            float fu = fns[p] * powf(2.0f, desafino[u] / 1200.0f);
            float w = 2.0f * (float)M_PI * fu / taxa;
            v->c[i] = 1.0f;
            v->s[i] = 0.0f;
            v->cr[i] = cosf(w);
            v->sr[i] = sinf(w);
            v->a1[i] = ganho * amps[p] * 0.7f;
            v->a2[i] = ganho * amps[p] * 0.3f;
            v->d1[i] = m1;
            v->d2[i] = m2;
        }
    }
    v->n_osc = i;

    v->ruido_amp = 0.04f * vel * vel;
    v->ruido_dec = expf(-1.0f / (0.004f * taxa));

    /* pan: graves à esquerda, agudos à direita (como o pianista ouve) */
    float pan = ((float)tecla / (NUM_TECLAS - 1) - 0.5f) * 1.2f;
    float ang = (pan + 1.0f) * (float)M_PI / 4.0f;
    v->ganho_e = cosf(ang);
    v->ganho_d = sinf(ang);
}

static void abafar(Synth *s, Voz *v)
{
    if (v->abafada || v->tecla >= TECLA_SEM_ABAFADOR) return;
    float m = expf(-1.0f / (TAU_ABAFADO * s->taxa));
    for (int i = 0; i < v->n_osc; i++) {
        v->d1[i] = fminf(v->d1[i], m);
        v->d2[i] = fminf(v->d2[i], m);
    }
    v->abafada = 1;
}

void synth_nota_off(Synth *s, int tecla)
{
    for (int i = 0; i < MAX_VOZES; i++)
        if (s->vozes[i].ativa && s->vozes[i].tecla == tecla)
            abafar(s, &s->vozes[i]);
}

void synth_abafar_tudo(Synth *s)
{
    for (int i = 0; i < MAX_VOZES; i++)
        if (s->vozes[i].ativa)
            abafar(s, &s->vozes[i]);
}

static void render_voz(Voz *v, float *mono, int n)
{
    const int no = v->n_osc;
    float *restrict c = v->c, *restrict sn = v->s;
    const float *restrict cr = v->cr, *restrict sr = v->sr;
    float *restrict a1 = v->a1, *restrict a2 = v->a2;
    const float *restrict d1 = v->d1, *restrict d2 = v->d2;

    for (int t = 0; t < n; t++) {
        float acc = 0.0f;
        for (int i = 0; i < no; i++) {
            float nc = c[i] * cr[i] - sn[i] * sr[i];
            float ns = c[i] * sr[i] + sn[i] * cr[i];
            c[i] = nc;
            sn[i] = ns;
            acc += (a1[i] + a2[i]) * ns;
            a1[i] *= d1[i];
            a2[i] *= d2[i];
        }
        if (v->ruido_amp > 1e-6f) {
            v->rng = v->rng * 1664525u + 1013904223u;
            float r = (float)(int)v->rng * (1.0f / 2147483648.0f);
            v->ruido_lp += 0.25f * (r - v->ruido_lp);
            acc += v->ruido_lp * v->ruido_amp;
            v->ruido_amp *= v->ruido_dec;
        }
        mono[t] = acc;
    }

    /* corrige deriva numérica do módulo do fasor */
    float energia = 0.0f;
    for (int i = 0; i < no; i++) {
        float g = 1.5f - 0.5f * (c[i] * c[i] + sn[i] * sn[i]);
        c[i] *= g;
        sn[i] *= g;
        energia += a1[i] + a2[i];
    }
    if (energia < 1e-5f && v->ruido_amp < 1e-6f)
        v->ativa = 0;
}

void synth_render(Synth *s, float *saida, int n)
{
    float mono[BLOCO];
    float cavalete[BLOCO];
    float tampo[2 * BLOCO];
    memset(saida, 0, sizeof(float) * 2 * n);

    for (int ini = 0; ini < n; ini += BLOCO) {
        int m = n - ini < BLOCO ? n - ini : BLOCO;
        float *out = saida + 2 * ini;
        memset(cavalete, 0, sizeof cavalete);
        for (int k = 0; k < MAX_VOZES; k++) {
            Voz *v = &s->vozes[k];
            if (!v->ativa) continue;
            render_voz(v, mono, m);
            for (int t = 0; t < m; t++) {
                out[2 * t]     += mono[t] * v->ganho_e;
                out[2 * t + 1] += mono[t] * v->ganho_d;
                cavalete[t]    += mono[t];
            }
        }

        /* todas as cordas excitam o mesmo cavalete -> tampo -> estéreo */
        if (s->tampo && m == BLOCO) {
            tampo_processar(s->tampo, cavalete, tampo);
            for (int i = 0; i < 2 * m; i++)
                out[i] = out[i] * s->mix_seco + tampo[i] * s->ganho_tampo;
        }
    }

    /* saturação suave para muitos acordes simultâneos */
    for (int i = 0; i < 2 * n; i++) {
        float x = saida[i] * s->volume;
        x = clampf(x, -3.0f, 3.0f);
        saida[i] = x * (27.0f + x * x) / (27.0f + 9.0f * x * x);
    }
}

int synth_vozes_ativas(const Synth *s)
{
    int n = 0;
    for (int i = 0; i < MAX_VOZES; i++)
        n += s->vozes[i].ativa;
    return n;
}
