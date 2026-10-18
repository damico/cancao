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

//! Modelo de guia de onda digital: simula a onda que viaja pela corda.
//!
//! O golpe do martelo cria duas ondas que correm em sentidos opostos com
//! velocidade c = √(T/μ), refletem nas pontas (agrafe e cavalete) e voltam.
//! Uma volta completa leva 2L/c segundos, então o som se repete com
//!
//! ```text
//! f = c / 2L = (1 / 2L) * √(T / μ)
//! ```
//!
//! No computador, a volta vira uma linha de atraso de `taxa / f` amostras
//! fechada em laço. A cada volta o sinal passa por:
//!
//! - um filtro de perdas (passa-baixa de um polo): a corda perde energia
//!   para o ar e para a madeira, mais rápido nas frequências altas;
//! - um passa-tudo de atraso fracionário: `taxa / f` raramente é inteiro,
//!   e ele completa o atraso para a afinação ficar exata.
//!
//! O martelo injeta um pulso de força a 1/8 do comprimento. A onda que vai
//! para a ponta mais próxima volta invertida e atrasada, e isso apaga
//! sozinho os harmônicos múltiplos de 8, sem fórmula.
//!
//! Etapa 1 do roteiro: ainda sem rigidez (inarmonicidade), sem martelo não
//! linear e sem acoplamento das cordas no cavalete.

use std::f32::consts::PI;

use crate::corda::Corda;
use crate::modal::tau_base;
use crate::synth::{BLOCO, DESAFINO_UNISSONO, MAX_UNISSONO, POS_MARTELO, SIMPATIA_UNA_CORDA, TAU_ABAFADO};

/// Atraso máximo de uma volta: A0 a 48 kHz precisa de ~1745 amostras.
const MAX_ATRASO: usize = 2048;
/// Pulso do martelo (até ~4 ms) mais a reflexão (até ~1/8 da volta).
const MAX_EXCITACAO: usize = 1024;

/// Uma corda do uníssono.
#[derive(Clone, Copy)]
struct Corda1 {
    linha: [f32; MAX_ATRASO],
    pos: usize,
    /// parte inteira do atraso
    n: usize,
    /// filtro de perdas: y = b*x − a*y₋₁, com b = g*(1 + a)
    a: f32,
    b: f32,
    g_natural: f32,
    lp_y1: f32,
    /// passa-tudo fracionário: y = η*x + x₋₁ − η*y₋₁
    eta: f32,
    ap_x1: f32,
    ap_y1: f32,
    /// quanto do golpe esta corda recebe (una corda: a corda não atingida
    /// recebe pouco)
    golpe: f32,
}

impl Default for Corda1 {
    fn default() -> Self {
        Corda1 {
            linha: [0.0; MAX_ATRASO],
            pos: 0,
            n: 2,
            a: 0.0,
            b: 0.0,
            g_natural: 0.0,
            lp_y1: 0.0,
            eta: 0.0,
            ap_x1: 0.0,
            ap_y1: 0.0,
            golpe: 0.0,
        }
    }
}

impl Corda1 {
    /// `f`: frequência desta corda; `tau0`: decaimento da fundamental (s)
    fn new(f: f32, tau0: f32, golpe: f32, taxa: f32) -> Self {
        let w0 = 2.0 * PI * f / taxa;
        let (a, g) = ajustar_perdas(f, tau0, taxa);

        // atraso total de uma volta = taxa / f; o filtro de perdas já atrasa
        // um pouco, e o passa-tudo completa a fração que falta
        let atraso_lp = (-a * w0.sin()).atan2(1.0 + a * w0.cos()) / w0;
        let resto = taxa / f - atraso_lp;
        let n = ((resto - 0.5).floor() as usize).clamp(2, MAX_ATRASO - 1);
        let delta = resto - n as f32; // entre 0,5 e 1,5: passa-tudo bem comportado
        // atraso de fase exatamente delta na fundamental
        let eta = ((1.0 - delta) * w0 / 2.0).sin() / ((1.0 + delta) * w0 / 2.0).sin();

        Corda1 { n, a, b: g * (1.0 + a), g_natural: g, eta, golpe, ..Corda1::default() }
    }

    fn abafador(&mut self, fator: f32) {
        self.b = self.g_natural * fator * (1.0 + self.a);
    }

    #[inline]
    fn amostra(&mut self, entrada: f32) -> f32 {
        let x = self.linha[self.pos];
        let lp = self.b * x - self.a * self.lp_y1;
        self.lp_y1 = lp;
        let ap = self.eta * lp + self.ap_x1 - self.eta * self.ap_y1;
        self.ap_x1 = lp;
        self.ap_y1 = ap;
        let v = ap + entrada * self.golpe;
        self.linha[self.pos] = v;
        self.pos += 1;
        if self.pos == self.n {
            self.pos = 0;
        }
        v
    }
}

/// Filtro de perdas H(z) = g*(1+a) / (1 + a*z⁻¹) tal que, por volta na corda,
/// a fundamental decai com `tau0` e uma frequência 3 kHz acima dela decai
/// duas vezes mais rápido (como no modelo modal). Devolve (a, g).
fn ajustar_perdas(f: f32, tau0: f32, taxa: f32) -> (f32, f32) {
    let w0 = 2.0 * PI * f / taxa;
    let w1 = (2.0 * PI * (f + 3000.0) / taxa).min(0.95 * PI);
    // ganho por volta (uma volta dura 1/f segundos)
    let g0 = (-1.0 / (tau0 * f)).exp();
    let g1 = (-2.0 / (tau0 * f)).exp();
    let alvo = g1 / g0;
    let den = |a: f32, w: f32| (1.0 + 2.0 * a * w.cos() + a * a).sqrt();
    let razao = |a: f32| den(a, w0) / den(a, w1);

    // razao(0) = 1 e cai à medida que a tende a −1: bisseção
    let (mut lo, mut hi) = (-0.999f32, 0.0f32);
    for _ in 0..40 {
        let m = 0.5 * (lo + hi);
        if razao(m) > alvo { hi = m } else { lo = m }
    }
    let a = 0.5 * (lo + hi);
    let g = (g0 * den(a, w0) / (1.0 + a)).min(0.99999);
    (a, g)
}

#[derive(Clone, Copy)]
pub struct VozGuia {
    cordas: [Corda1; MAX_UNISSONO],
    nu: usize,
    excitacao: [f32; MAX_EXCITACAO],
    len_exc: usize,
    t: usize,
    f: f32,
    /// pico da saída com decaimento lento (escala de uma volta): no grave a
    /// onda é um pulso curto seguido de silêncio a cada volta, e só o bloco
    /// atual não diz se a corda ainda vibra
    envoltoria: f32,
    queda_envoltoria: f32,
}

impl Default for VozGuia {
    fn default() -> Self {
        VozGuia {
            cordas: [Corda1::default(); MAX_UNISSONO],
            nu: 0,
            excitacao: [0.0; MAX_EXCITACAO],
            len_exc: 0,
            t: 0,
            f: 440.0,
            envoltoria: 0.0,
            queda_envoltoria: 0.0,
        }
    }
}

impl VozGuia {
    /// `ganho`: volume da nota (força, compensação dos agudos).
    pub fn new(c: &Corda, vel: f32, una_corda: f32, ganho: f32, taxa: f32) -> Self {
        let f = c.f as f32;
        let mut v = VozGuia { nu: c.n_cordas, f, ..VozGuia::default() };

        // sem o acoplamento das cordas (etapa 4) não há decaimento em dois
        // estágios; um único decaimento entre os dois do modelo modal
        let tau0 = 2.5 * tau_base(f);
        let atingidas = if una_corda >= 0.5 && v.nu > 1 { v.nu - 1 } else { v.nu };
        for (u, &cents) in DESAFINO_UNISSONO[..v.nu].iter().enumerate() {
            let golpe = if u < atingidas { 1.0 } else { SIMPATIA_UNA_CORDA };
            v.cordas[u] = Corda1::new(f * 2f32.powf(cents / 1200.0), tau0, golpe, taxa);
        }

        // martelo: pulso de força de ~4 ms no A0 a ~0,5 ms no C8; mais curto
        // quando mais forte (feltro comprimido = mais duro = mais brilho);
        // com una corda bate com feltro mais macio
        let contato_s = 4e-3 * (f / 27.5).powf(-0.414) * (1.3 - 0.6 * vel) * (1.0 + 0.4 * una_corda);
        let n_pulso = ((contato_s * taxa) as usize).clamp(2, MAX_EXCITACAO / 2);
        let periodo = taxa / f;
        // a reflexão na ponta mais próxima do martelo volta invertida, depois
        // de percorrer 2 × (posição do martelo) = POS_MARTELO de uma volta
        let reflexo = ((POS_MARTELO * periodo).round() as usize).clamp(1, MAX_EXCITACAO / 2);
        v.len_exc = n_pulso + reflexo;
        for i in 0..n_pulso {
            let p = 0.5 * (1.0 - (2.0 * PI * i as f32 / n_pulso as f32).cos());
            v.excitacao[i] += p;
            v.excitacao[i + reflexo] -= p;
        }

        // volume: a corda repete a excitação a cada volta, como um sinal
        // periódico. Para uma soma de senoides, Σ amplitude² = 2 × potência,
        // a mesma normalização do modal
        let exc = &v.excitacao[..v.len_exc];
        let potencia = if (v.len_exc as f32) <= periodo {
            // a excitação cabe numa volta: potência = energia / período (Parseval)
            exc.iter().map(|x| x * x).sum::<f32>() / periodo
        } else {
            // agudos: o golpe dura mais que uma volta e se sobrepõe a si mesmo;
            // soma a potência dos harmônicos verdadeiros (poucos nos agudos)
            let mut p = 0f32;
            let mut k = 1;
            while k as f32 * f < 0.5 * taxa {
                let w = 2.0 * PI * k as f32 * f / taxa;
                let (re, im) = exc.iter().enumerate().fold((0f32, 0f32), |(re, im), (i, &x)| {
                    (re + x * (w * i as f32).cos(), im - x * (w * i as f32).sin())
                });
                // amplitude do harmônico k = 2 |E(ω_k)| / período
                p += 2.0 * (re * re + im * im) / (periodo * periodo);
                k += 1;
            }
            p
        };
        let norm = if potencia > 0.0 { 1.0 / (2.0 * potencia).sqrt() / v.nu as f32 } else { 0.0 };
        for x in &mut v.excitacao[..v.len_exc] {
            *x *= ganho * norm;
        }
        v.queda_envoltoria = (-1.0 / periodo).exp();
        v
    }

    /// Posição do abafador: 0 = levantado, 1 = encostado.
    pub fn abafador(&mut self, contato: f32, _taxa: f32) {
        // perda extra por volta (1/f segundos) com o abafador encostado
        let fator = (-contato / (TAU_ABAFADO * self.f)).exp();
        for c in &mut self.cordas[..self.nu] {
            c.abafador(fator);
        }
    }

    /// Escreve um bloco mono; devolve uma medida da energia restante.
    pub fn render(&mut self, mono: &mut [f32; BLOCO]) -> f32 {
        for m in mono.iter_mut() {
            let e = if self.t < self.len_exc { self.excitacao[self.t] } else { 0.0 };
            self.t += 1;
            let mut s = 0f32;
            for c in &mut self.cordas[..self.nu] {
                s += c.amostra(e);
            }
            *m = s;
            self.envoltoria = s.abs().max(self.envoltoria * self.queda_envoltoria);
        }
        // ainda golpeando, ou a onda ainda não completou a primeira volta (a
        // saída fica em silêncio enquanto ela viaja): não deixa desligar
        let primeira_volta = self.len_exc + self.cordas[0].n;
        if self.t < primeira_volta { 1.0 } else { self.envoltoria }
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    /// atraso de fase de y = η*x + x₋₁ − η*y₋₁ em ω (amostras)
    fn atraso_passa_tudo(eta: f32, w: f32) -> f32 {
        // A = (η + e^{-jω}) / (1 + η e^{-jω}): fase do numerador menos a do denominador
        let fase = (-w.sin()).atan2(eta + w.cos()) - (-eta * w.sin()).atan2(1.0 + eta * w.cos());
        -fase / w
    }

    #[test]
    fn laco_tem_exatamente_uma_volta_de_atraso() {
        // parte inteira + filtro de perdas + passa-tudo = taxa / f, em toda a extensão
        let taxa = 48000.0;
        for k in 0..88 {
            let f = 27.5 * 2f32.powf(k as f32 / 12.0);
            let c = Corda1::new(f, 2.5 * tau_base(f), 1.0, taxa);
            let w0 = 2.0 * PI * f / taxa;
            let atraso_lp = (-c.a * w0.sin()).atan2(1.0 + c.a * w0.cos()) / w0;
            let total = c.n as f32 + atraso_lp + atraso_passa_tudo(c.eta, w0);
            let erro = total - taxa / f;
            assert!(erro.abs() < 2e-3 * (taxa / f) / 100.0 + 1e-3, "tecla {k} ({f:.1} Hz): erro {erro} amostras");
        }
    }

    #[test]
    fn perdas_ajustadas_decaem_como_pedido() {
        for f in [27.5f32, 110.0, 440.0, 1760.0, 4186.0] {
            let taxa = 48000.0;
            let tau0 = 2.5 * tau_base(f);
            let (a, g) = ajustar_perdas(f, tau0, taxa);
            let ganho = |w: f32| g * (1.0 + a) / (1.0 + 2.0 * a * w.cos() + a * a).sqrt();
            let w0 = 2.0 * PI * f / taxa;
            // por volta: exp(-1/(tau f)) => tau = -1/(f ln ganho)
            let tau_medido = -1.0 / (f * ganho(w0).ln());
            assert!((tau_medido / tau0 - 1.0).abs() < 1e-2, "{f} Hz: tau {tau_medido} vs {tau0}");
            assert!(g < 1.0 && (-1.0..=0.0).contains(&a));
        }
    }
}
