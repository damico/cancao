// Copyright 2026 José Damico <jose.damico@scicrop.com>
// SPDX-License-Identifier: Apache-2.0
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Modelo modal: cada nota é um conjunto de osciladores senoidais, um por
//! parcial e por corda do uníssono. Imita o resultado da física:
//!
//! - frequências: f_n = n f √(1 + B n²), com f vindo de f = 1/(2L) √(T/μ)
//! - amplitudes: martelo batendo a ~1/8 do comprimento dá sin(n π / 8), e o
//!   feltro do martelo agindo como passa-baixa (mais forte = mais brilho)
//! - decaimento: graves soam por mais tempo; parciais agudos morrem antes;
//!   dois estágios (som "imediato" + "pós-som")

use std::f32::consts::PI;

use crate::corda::Corda;
use crate::synth::{BLOCO, DESAFINO_UNISSONO, MAX_UNISSONO, POS_MARTELO, SIMPATIA_UNA_CORDA, TAU_ABAFADO};

pub const MAX_PARCIAIS: usize = 48;

/// Osciladores ficam em grupos de 8: com 8 acumuladores independentes o
/// compilador gera SIMD sem precisar reordenar somas de ponto flutuante.
const LARGURA: usize = 8;
const MAX_GRUPOS: usize = (MAX_PARCIAIS * MAX_UNISSONO).div_ceil(LARGURA);

#[derive(Clone, Copy, Default)]
struct Grupo {
    c: [f32; LARGURA], // fasor (cos, sen)
    s: [f32; LARGURA],
    cr: [f32; LARGURA], // rotação por amostra
    sr: [f32; LARGURA],
    a1: [f32; LARGURA], // envelope em 2 estágios
    a2: [f32; LARGURA],
    d1: [f32; LARGURA], // decaimento por amostra, com o abafador atual
    d2: [f32; LARGURA],
    d1n: [f32; LARGURA], // decaimento natural da corda livre
    d2n: [f32; LARGURA],
}

#[derive(Clone, Copy, Default)]
pub struct VozModal {
    n_grupos: usize,
    grupos: [Grupo; MAX_GRUPOS],
}

impl VozModal {
    /// `ganho`: volume da nota (força, compensação dos agudos).
    pub fn new(c: &Corda, vel: f32, una_corda: f32, ganho: f32, taxa: f32) -> Self {
        let f = c.f as f32;
        let f_max = (0.45 * taxa).min(12000.0);

        // feltro: corte do passa-baixa sobe com a velocidade; com una corda o
        // martelo bate com uma parte menos compactada do feltro (mais escuro)
        let fc = (f * (2.0 + 10.0 * vel * vel)).max(300.0 + 3500.0 * vel * vel) * (1.0 - 0.3 * una_corda);

        // decaimento base: ~4 s no A0, ~0,8 s no A4, ~0,2 s no C8 (1º estágio);
        // o 2º estágio é 6x mais longo
        let tau1 = tau_base(f);
        let tau2 = tau1 * 6.0;

        let nu = c.n_cordas;
        // una corda: o martelo acerta uma corda a menos; a outra só vibra por
        // simpatia, pelo cavalete. A normalização continua por `nu`, então a
        // nota fica naturalmente mais fraca (menos cordas recebendo energia)
        let atingidas = if una_corda >= 0.5 && nu > 1 { nu - 1 } else { nu };

        let mut amps = [0f32; MAX_PARCIAIS];
        let mut fns = [0f32; MAX_PARCIAIS];
        let mut np = 0;
        let mut soma2 = 0f64;
        for n in 1..=MAX_PARCIAIS as u32 {
            let fn_ = c.parcial(n) as f32;
            if fn_ >= f_max {
                break;
            }
            let mut a = (n as f32 * PI * POS_MARTELO).sin().abs() / (n as f32).powf(0.7);
            a /= 1.0 + (fn_ / fc) * (fn_ / fc);
            amps[np] = a;
            fns[np] = fn_;
            soma2 += (a * a) as f64;
            np += 1;
        }
        let norm = if soma2 > 0.0 { 1.0 / (soma2 as f32).sqrt() / nu as f32 } else { 0.0 };
        let ganho = ganho * norm;

        let mut v = VozModal::default();
        let mut i = 0;
        for p in 0..np {
            // parciais acima da fundamental morrem antes
            let r = (fns[p] - fns[0]) / 3000.0;
            let tau_p = 1.0 / (1.0 + r * r);
            let m1 = (-1.0 / (tau1 * tau_p * taxa)).exp();
            let m2 = (-1.0 / (tau2 * tau_p * taxa)).exp();
            for (u, &cents) in DESAFINO_UNISSONO[..nu].iter().enumerate() {
                let simpatia = if u < atingidas {
                    1.0
                } else {
                    let n = (p + 1) as f32 / 4.0;
                    SIMPATIA_UNA_CORDA / (1.0 + n * n)
                };
                let fu = fns[p] * 2f32.powf(cents / 1200.0);
                let w = 2.0 * PI * fu / taxa;
                let g = &mut v.grupos[i / LARGURA];
                let l = i % LARGURA;
                g.c[l] = 1.0;
                g.s[l] = 0.0;
                g.cr[l] = w.cos();
                g.sr[l] = w.sin();
                g.a1[l] = ganho * amps[p] * simpatia * 0.7;
                g.a2[l] = ganho * amps[p] * simpatia * 0.3;
                g.d1[l] = m1;
                g.d2[l] = m2;
                g.d1n[l] = m1;
                g.d2n[l] = m2;
                i += 1;
            }
        }
        v.n_grupos = i.div_ceil(LARGURA);
        v
    }

    /// Posição do abafador: 0 = levantado, 1 = encostado.
    pub fn abafador(&mut self, contato: f32, taxa: f32) {
        let fd = (-contato / (TAU_ABAFADO * taxa)).exp();
        for g in &mut self.grupos[..self.n_grupos] {
            for l in 0..LARGURA {
                g.d1[l] = g.d1n[l] * fd;
                g.d2[l] = g.d2n[l] * fd;
            }
        }
    }

    /// Escreve um bloco mono; devolve uma medida da energia restante.
    pub fn render(&mut self, mono: &mut [f32; BLOCO]) -> f32 {
        let grupos = &mut self.grupos[..self.n_grupos];
        for m in mono.iter_mut() {
            let mut acc = [0f32; LARGURA];
            for g in grupos.iter_mut() {
                for l in 0..LARGURA {
                    let nc = g.c[l] * g.cr[l] - g.s[l] * g.sr[l];
                    let ns = g.c[l] * g.sr[l] + g.s[l] * g.cr[l];
                    g.c[l] = nc;
                    g.s[l] = ns;
                    acc[l] += (g.a1[l] + g.a2[l]) * ns;
                    g.a1[l] *= g.d1[l];
                    g.a2[l] *= g.d2[l];
                }
            }
            *m = acc.iter().sum();
        }

        // corrige deriva numérica do módulo do fasor
        let mut energia = 0f32;
        for g in grupos.iter_mut() {
            for l in 0..LARGURA {
                let k = 1.5 - 0.5 * (g.c[l] * g.c[l] + g.s[l] * g.s[l]);
                g.c[l] *= k;
                g.s[l] *= k;
                energia += g.a1[l] + g.a2[l];
            }
        }
        energia
    }
}

/// Decaimento do 1º estágio da fundamental: ~4 s no A0, ~0,8 s no A4, ~0,2 s no C8.
pub fn tau_base(f: f32) -> f32 {
    4.0 * (f / 27.5).powf(-0.6)
}
