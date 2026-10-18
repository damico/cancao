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
 * tampo.h - tampo harmônico + sala por convolução.
 *
 * As cordas quase não movem ar: quem irradia o som é o tampo, excitado
 * pelo cavalete. Modelamos isso como um sistema linear: a soma das
 * cordas (mono, no cavalete) é convoluída com a resposta ao impulso
 * do tampo + sala, uma para cada ouvido (estéreo).
 *
 * Convolução particionada uniforme (overlap-save) com blocos de
 * TAMPO_BLOCO amostras: custo constante por bloco e nenhuma latência
 * além do próprio bloco de áudio.
 */
#ifndef TAMPO_H
#define TAMPO_H

#define TAMPO_BLOCO 128

typedef struct Tampo Tampo;

/* Resposta ao impulso sintética (modos de placa + cauda de sala). */
Tampo *tampo_sintetico(float taxa, float duracao_s, unsigned semente);

/* Resposta ao impulso de um WAV (PCM 16 bits ou float 32, mono ou estéreo). */
Tampo *tampo_de_wav(const char *caminho, float taxa, float duracao_max_s);

/* Processa exatamente TAMPO_BLOCO amostras: entrada mono, saída estéreo
 * intercalada (sobrescreve). */
void tampo_processar(Tampo *t, const float *entrada, float *saida);

/* Grava a resposta ao impulso em WAV estéreo (para ouvir/inspecionar). */
int tampo_salvar_ir(const Tampo *t, const char *caminho, int taxa);

void tampo_liberar(Tampo *t);

#endif
