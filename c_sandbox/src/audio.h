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
 * audio.h - saída de áudio em tempo real (ALSA).
 */
#ifndef AUDIO_H
#define AUDIO_H

typedef struct Audio Audio;

/* latencia_us: buffer pedido ao driver (ex.: 10000 = 10 ms) */
Audio *audio_abrir(const char *dispositivo, unsigned taxa, unsigned latencia_us);

/* Bloqueia até o driver aceitar os n quadros estéreo (float -1..1). */
int audio_escrever(Audio *a, const float *quadros, int n);

void audio_fechar(Audio *a);

#endif
