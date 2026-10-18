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
//! O martelo (ver [`crate::martelo`]) bate a 1/8 do comprimento. A onda que
//! vai para a ponta mais próxima volta invertida e atrasada, e isso apaga
//! sozinho os harmônicos múltiplos de 8, sem fórmula.
//!
//! Rigidez (etapa 2): num fio de aço, as frequências altas viajam mais rápido
//! (dispersão) e os parciais ficam esticados, f_n = n f √(1 + B n²). Uma
//! cascata de passa-tudos de 1ª ordem no laço tem atraso que diminui com a
//! frequência: os agudos dão a volta mais depressa. O número de seções e o
//! coeficiente são escolhidos por tecla para os parciais caírem em f_n.
//!
//! Cavalete (etapa 4): as 2 ou 3 cordas de uma nota terminam no mesmo
//! cavalete, que se move com a soma das ondas que chegam e devolve esse
//! movimento a todas. Cordas em fase empurram o cavalete juntas e perdem
//! energia rápido (o som imediato); em oposição a soma se cancela e elas soam
//! por muito tempo (o pós-som). A desafinação mínima entre elas passa energia
//! de um modo para o outro aos poucos: o decaimento em dois estágios do piano
//! sai daí (Weinreich, 1977). Com una corda, a corda não atingida só vibra
//! porque o cavalete a excita.
//!
//! Ainda sem a ressonância simpática entre notas diferentes.

use std::f32::consts::PI;

use crate::corda::Corda;
use crate::martelo::{self, MAX_GOLPE};
use crate::modal::tau_base;
use crate::synth::{BLOCO, MAX_UNISSONO, POS_MARTELO, TAU_ABAFADO};

/// Atraso máximo de uma volta: A0 a 48 kHz precisa de ~1745 amostras.
const MAX_ATRASO: usize = 2048;
/// Golpe do martelo mais a reflexão (até ~1/8 da volta).
const MAX_EXCITACAO: usize = MAX_GOLPE + MAX_ATRASO / 4;
/// Desafinação entre as cordas do uníssono (cents). Os dois estágios do
/// decaimento só aparecem quando ela é pequena perto do acoplamento pelo
/// cavalete; com os 0,9 e −0,6 cents do modelo modal, nos médios e agudos
/// (onde o mesmo cent vale mais Hz) as cordas decaem quase juntas e o pós-som
/// some (medido: A4 caindo 7 a 9 dB/s no fim, em vez de ~2).
const DESAFINO_GUIA: [f32; MAX_UNISSONO] = [0.0, 0.2, -0.13];
/// Seções do filtro de dispersão.
pub const MAX_DISPERSAO: usize = 32;
/// Parciais acima disto não entram no projeto da dispersão (nos agudos o
/// limite sobe para incluir pelo menos o 2º e o 3º parcial).
const F_MAX_DISPERSAO: f32 = 5000.0;

fn limite_dispersao(f1: f32, taxa: f32) -> f32 {
    F_MAX_DISPERSAO.max(3.5 * f1).min(0.45 * taxa)
}

/// Filtro de dispersão de uma tecla: `m` passa-tudos iguais de coeficiente `a`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Dispersao {
    pub m: usize,
    pub a: f32,
}

/// Atraso de fase (amostras) de y = a*x + x₋₁ − a*y₋₁ na frequência w (rad/amostra).
fn atraso_passa_tudo(a: f32, w: f32) -> f32 {
    // A = (a + e^{-jω}) / (1 + a e^{-jω}): fase do numerador menos a do denominador
    let fase = (-w.sin()).atan2(a + w.cos()) - (-a * w.sin()).atan2(1.0 + a * w.cos());
    -fase / w
}

/// Atraso de fase (amostras) do filtro de perdas g*(1+a) / (1 + a*z⁻¹).
fn atraso_perdas(a: f32, w: f32) -> f32 {
    (-a * w.sin()).atan2(1.0 + a * w.cos()) / w
}

/// Afinação da volta: com o atraso dos filtros de perdas e de dispersão na
/// fundamental, devolve a parte inteira da linha e o coeficiente η do
/// passa-tudo fracionário que completa exatamente `taxa / f`.
pub(crate) fn sintonia(f: f32, a_perdas: f32, disp: Dispersao, taxa: f32) -> (usize, f32) {
    let w0 = 2.0 * PI * f / taxa;
    let filtros = atraso_perdas(a_perdas, w0) + disp.m as f32 * atraso_passa_tudo(disp.a, w0);
    let resto = taxa / f - filtros;
    let n = ((resto - 0.5).floor() as usize).clamp(2, MAX_ATRASO - 1);
    let delta = resto - n as f32; // entre 0,5 e 1,5: passa-tudo bem comportado
    // atraso de fase exatamente delta na fundamental
    let eta = ((1.0 - delta) * w0 / 2.0).sin() / ((1.0 + delta) * w0 / 2.0).sin();
    (n, eta)
}

/// Atraso de fase de uma volta completa do laço na frequência w.
fn atraso_volta(n: usize, a_perdas: f32, disp: Dispersao, eta: f32, w: f32) -> f32 {
    n as f32 + atraso_perdas(a_perdas, w) + disp.m as f32 * atraso_passa_tudo(disp.a, w) + atraso_passa_tudo(eta, w)
}

/// Erro tolerado nos parciais: abaixo disto a diferença é inaudível, e mais
/// seções só custariam processamento.
const TOLERANCIA_CENTS: f32 = 1.5;

/// Decaimento próprio da corda (ar, material), sem o cavalete. Com 2 ou 3
/// cordas ele é o pós-som, 6 vezes o decaimento base (como o 2º estágio do
/// modal); o cavalete acrescenta a perda rápida do som imediato. Notas de
/// uma corda só não têm o par em oposição (o pós-som delas vem das duas
/// polarizações da corda, ainda não modeladas) e ficam com um decaimento
/// único, entre os dois estágios do modal.
pub(crate) fn tau_corda(c: &Corda) -> f32 {
    let f = c.parcial(1) as f32;
    if c.n_cordas > 1 { 6.0 * tau_base(f) } else { 2.5 * tau_base(f) }
}

/// Acoplamento pelo cavalete: a cada volta, cada corda perde `α` vezes a soma
/// das ondas que chegaram. As cordas em fase decaem então com
/// (1 − n α) por volta, além da perda própria; α é escolhido para esse modo
/// decair com o decaimento base (o 1º estágio do modal). Os autovalores da
/// junção são 1 e 1 − n α, então ela nunca cria energia.
fn acoplamento(c: &Corda) -> f32 {
    if c.n_cordas < 2 {
        return 0.0;
    }
    let f = c.parcial(1) as f32;
    let extra = 1.0 / tau_base(f) - 1.0 / tau_corda(c);
    (1.0 - (-extra / f).exp()) / c.n_cordas as f32
}

/// Escolhe o filtro de dispersão da corda: para cada número de seções, o
/// coeficiente que minimiza o erro (cents, RMS) dos parciais abaixo de
/// 5 kHz; fica o menor número de seções com erro tolerável. O atraso extra
/// precisa caber numa volta da corda, o que limita os agudos. Caro demais
/// para fazer a cada nota: calcula-se uma vez por tecla.
pub fn projetar_dispersao(c: &Corda, taxa: f32) -> Dispersao {
    let f1 = c.parcial(1) as f32;
    let (a_lp, _) = ajustar_perdas(f1, tau_corda(c), taxa);
    let w = |n: u32| 2.0 * PI * c.parcial(n) as f32 / taxa;
    let f_lim = limite_dispersao(f1, taxa);
    let parciais: Vec<u32> = (2..=40).filter(|&n| (c.parcial(n) as f32) < f_lim).collect();
    if parciais.is_empty() {
        return Dispersao::default();
    }
    let w1 = w(1);

    // laço completo, como a corda o monta (inclusive a dispersão do
    // passa-tudo fracionário, que pesa nos agudos)
    let erro = |m: usize, a: f32| -> f32 {
        let disp = Dispersao { m, a };
        if atraso_perdas(a_lp, w1) + m as f32 * atraso_passa_tudo(a, w1) + 2.5 > taxa / f1 {
            return f32::INFINITY; // não cabe numa volta
        }
        let (n_int, eta) = sintonia(f1, a_lp, disp, taxa);
        let soma: f32 = parciais
            .iter()
            .map(|&n| {
                let alvo = taxa * n as f32 / c.parcial(n) as f32;
                let cents = 1200.0 * (alvo / atraso_volta(n_int, a_lp, disp, eta, w(n))).log2();
                cents * cents
            })
            .sum();
        (soma / parciais.len() as f32).sqrt()
    };

    // melhor coeficiente para cada número de seções
    let mut opcoes = vec![(erro(0, 0.0), Dispersao::default())];
    for m in 1..=MAX_DISPERSAO {
        // busca da seção áurea em a ∈ [−0,99, 0], reaproveitando um ponto
        let r = 0.618_034f32;
        let (mut lo, mut hi) = (-0.99f32, 0.0f32);
        let (mut x1, mut x2) = (hi - r * (hi - lo), lo + r * (hi - lo));
        let (mut e1, mut e2) = (erro(m, x1), erro(m, x2));
        for _ in 0..30 {
            if e1 < e2 {
                (hi, x2, e2) = (x2, x1, e1);
                x1 = hi - r * (hi - lo);
                e1 = erro(m, x1);
            } else {
                (lo, x1, e1) = (x1, x2, e2);
                x2 = lo + r * (hi - lo);
                e2 = erro(m, x2);
            }
        }
        let a = 0.5 * (lo + hi);
        opcoes.push((erro(m, a), Dispersao { m, a }));
    }
    let melhor_erro = opcoes.iter().map(|o| o.0).fold(f32::INFINITY, f32::min);
    let aceitavel = TOLERANCIA_CENTS.max(1.1 * melhor_erro);
    opcoes.into_iter().find(|o| o.0 <= aceitavel).map(|o| o.1).unwrap_or_default()
}

/// Linha de atraso de uma corda do uníssono.
#[derive(Clone, Copy)]
struct Linha {
    buf: [f32; MAX_ATRASO],
    pos: usize,
    /// parte inteira do atraso
    n: usize,
}

impl Default for Linha {
    fn default() -> Self {
        Linha { buf: [0.0; MAX_ATRASO], pos: 0, n: 2 }
    }
}

/// Faixas de processamento: as cordas do uníssono passam pelos filtros lado
/// a lado (SIMD), uma por faixa; faixas sem corda ficam zeradas.
const FAIXAS: usize = 4;
type Faixas = [f32; FAIXAS];

/// Filtro de perdas H(z) = g*(1+a) / (1 + a*z⁻¹) tal que, por volta na corda,
/// a fundamental decai com `tau0` e uma frequência 3 kHz acima dela decai
/// duas vezes mais rápido (como no modelo modal). Devolve (a, g).
pub(crate) fn ajustar_perdas(f: f32, tau0: f32, taxa: f32) -> (f32, f32) {
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

/// Nos agudos o contato do martelo dura mais que uma volta da corda, e a
/// maior parte da força cai entre os parciais: na corda essa parte se cancela
/// nas primeiras voltas, mas antes aparece como um estalo, que a normalização
/// do volume amplificaria muito (no C8 só ~0,3% da energia do golpe cai nos
/// parciais). Dobrar o golpe numa volta, somando os trechos deslocados de
/// período em período, faz esse cancelamento antes: o som sustentado é o
/// mesmo, sem o estalo.
fn dobrar_numa_volta(exc: &mut [f32; MAX_EXCITACAO], len: &mut usize, periodo: f32) {
    let original = *exc;
    let n = periodo.ceil() as usize;
    exc.fill(0.0);
    for (i, e) in exc[..n].iter_mut().enumerate() {
        // soma x(i + m*periodo), com interpolação linear no período fracionário
        let mut t = i as f32;
        while t < *len as f32 {
            let (k, frac) = (t as usize, t.fract());
            let prox = if k + 1 < *len { original[k + 1] } else { 0.0 };
            *e += original[k] * (1.0 - frac) + prox * frac;
            t += periodo;
        }
    }
    *len = n;
}

#[derive(Clone, Copy)]
pub struct VozGuia {
    linhas: [Linha; MAX_UNISSONO],
    nu: usize,
    // filtros de cada corda, uma por faixa
    /// perdas: y = b*x − a*y₋₁, com b = g*(1 + a)
    lp_a: Faixas,
    lp_b: Faixas,
    g_natural: Faixas,
    lp_y1: Faixas,
    /// dispersão: `disp.m` passa-tudos iguais (o mesmo para todas as cordas)
    disp: Dispersao,
    disp_x1: [Faixas; MAX_DISPERSAO],
    disp_y1: [Faixas; MAX_DISPERSAO],
    /// passa-tudo fracionário: y = η*x + x₋₁ − η*y₋₁
    eta: Faixas,
    ap_x1: Faixas,
    ap_y1: Faixas,
    /// quanto do golpe cada corda recebe (una corda: a não atingida, nada)
    golpe: Faixas,
    /// acoplamento pelo cavalete ([`acoplamento`])
    alfa: f32,
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
            linhas: [Linha::default(); MAX_UNISSONO],
            nu: 0,
            lp_a: [0.0; FAIXAS],
            lp_b: [0.0; FAIXAS],
            g_natural: [0.0; FAIXAS],
            lp_y1: [0.0; FAIXAS],
            disp: Dispersao::default(),
            disp_x1: [[0.0; FAIXAS]; MAX_DISPERSAO],
            disp_y1: [[0.0; FAIXAS]; MAX_DISPERSAO],
            eta: [0.0; FAIXAS],
            ap_x1: [0.0; FAIXAS],
            ap_y1: [0.0; FAIXAS],
            golpe: [0.0; FAIXAS],
            alfa: 0.0,
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
    /// `disp`: filtro de dispersão da tecla ([`projetar_dispersao`]).
    pub fn new(c: &Corda, disp: Dispersao, vel: f32, una_corda: f32, ganho: f32, taxa: f32) -> Self {
        // com rigidez a fundamental também sobe um pouco: f √(1 + B)
        let f = c.parcial(1) as f32;
        let mut v = VozGuia { nu: c.n_cordas, f, disp, alfa: acoplamento(c), ..VozGuia::default() };
        let tau0 = tau_corda(c);
        let atingidas = if una_corda >= 0.5 && v.nu > 1 { v.nu - 1 } else { v.nu };
        for (u, &cents) in DESAFINO_GUIA[..v.nu].iter().enumerate() {
            let fu = f * 2f32.powf(cents / 1200.0);
            let (a, g) = ajustar_perdas(fu, tau0, taxa);
            // atraso total de uma volta = taxa / f; os filtros de perdas e de
            // dispersão já atrasam, e o passa-tudo fracionário completa o resto
            let (n, eta) = sintonia(fu, a, disp, taxa);
            v.linhas[u].n = n;
            (v.lp_a[u], v.lp_b[u], v.g_natural[u]) = (a, g * (1.0 + a), g);
            v.eta[u] = eta;
            // a corda que o martelo não atinge só vibra pelo cavalete
            v.golpe[u] = if u < atingidas { 1.0 } else { 0.0 };
        }

        // martelo: a força sai da simulação do feltro contra a corda
        let mut forca = [0f32; MAX_GOLPE];
        let n_golpe = martelo::golpe(c, atingidas, vel, una_corda, taxa, &mut forca);
        let periodo = taxa / f;
        // a reflexão na ponta mais próxima do martelo volta invertida, depois
        // de percorrer 2 × (posição do martelo) = POS_MARTELO de uma volta
        let reflexo = ((POS_MARTELO * periodo).round() as usize).clamp(1, MAX_EXCITACAO - MAX_GOLPE);
        v.len_exc = n_golpe + reflexo;
        for (i, &p) in forca[..n_golpe].iter().enumerate() {
            v.excitacao[i] += p;
            v.excitacao[i + reflexo] -= p;
        }

        if v.len_exc as f32 > periodo {
            dobrar_numa_volta(&mut v.excitacao, &mut v.len_exc, periodo);
        }

        // volume: a corda repete a excitação a cada volta, como um sinal
        // periódico; potência = energia / período (Parseval). Para uma soma
        // de senoides, Σ amplitude² = 2 × potência, a mesma normalização do modal
        let potencia = v.excitacao[..v.len_exc].iter().map(|x| x * x).sum::<f32>() / periodo;
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
        for u in 0..self.nu {
            self.lp_b[u] = self.g_natural[u] * fator * (1.0 + self.lp_a[u]);
        }
    }

    /// Escreve um bloco mono; devolve uma medida da energia restante.
    pub fn render(&mut self, mono: &mut [f32; BLOCO]) -> f32 {
        let a = self.disp.a;
        for m in mono.iter_mut() {
            let e = if self.t < self.len_exc { self.excitacao[self.t] } else { 0.0 };
            self.t += 1;

            // o que volta de uma volta na corda, em cada faixa
            let mut d: Faixas = [0.0; FAIXAS];
            for u in 0..self.nu {
                let l = &self.linhas[u];
                d[u] = l.buf[l.pos];
            }
            // cavalete: move-se com a soma das ondas e devolve a todas
            let soma: f32 = d.iter().sum();
            for x in &mut d[..self.nu] {
                *x -= self.alfa * soma;
            }
            for u in 0..FAIXAS {
                d[u] = self.lp_b[u] * d[u] - self.lp_a[u] * self.lp_y1[u];
                self.lp_y1[u] = d[u];
            }
            for k in 0..self.disp.m {
                let (x1, y1) = (&mut self.disp_x1[k], &mut self.disp_y1[k]);
                for u in 0..FAIXAS {
                    let y = a * d[u] + x1[u] - a * y1[u];
                    x1[u] = d[u];
                    y1[u] = y;
                    d[u] = y;
                }
            }
            for u in 0..FAIXAS {
                let ap = self.eta[u] * d[u] + self.ap_x1[u] - self.eta[u] * self.ap_y1[u];
                self.ap_x1[u] = d[u];
                self.ap_y1[u] = ap;
                d[u] = ap + e * self.golpe[u];
            }

            let mut s = 0f32;
            for u in 0..self.nu {
                let l = &mut self.linhas[u];
                l.buf[l.pos] = d[u];
                l.pos += 1;
                if l.pos == l.n {
                    l.pos = 0;
                }
                s += d[u];
            }
            *m = s;
            self.envoltoria = s.abs().max(self.envoltoria * self.queda_envoltoria);
        }
        // ainda golpeando, ou a onda ainda não completou a primeira volta (a
        // saída fica em silêncio enquanto ela viaja): não deixa desligar
        let primeira_volta = self.len_exc + self.linhas[0].n;
        if self.t < primeira_volta { 1.0 } else { self.envoltoria }
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    use crate::corda;

    /// erro (cents) de cada parcial abaixo de 5 kHz em relação a f_n da fórmula
    fn erros_parciais(k: usize) -> Vec<f32> {
        let taxa = 48000.0;
        let cordas = corda::cordas(1.0);
        let c = &cordas[k];
        let f1 = c.parcial(1) as f32;
        let disp = projetar_dispersao(c, taxa);
        let (a, _) = ajustar_perdas(f1, tau_corda(c), taxa);
        let (n, eta) = sintonia(f1, a, disp, taxa);
        (1..=40)
            .map(|n| c.parcial(n) as f32)
            .take_while(|&f| f < limite_dispersao(f1, taxa))
            .enumerate()
            .map(|(i, f)| {
                let alvo = taxa * (i + 1) as f32 / f;
                1200.0 * (alvo / atraso_volta(n, a, disp, eta, 2.0 * PI * f / taxa)).log2()
            })
            .collect()
    }

    #[test]
    fn fundamental_exata_nas_88_teclas() {
        for k in 0..88 {
            let e = erros_parciais(k)[0];
            assert!(e.abs() < 0.05, "tecla {k}: fundamental com erro de {e} cents");
        }
    }

    #[test]
    fn parciais_seguem_a_inarmonicidade() {
        // sem dispersão os parciais ficariam até ~100 cents abaixo de f_n;
        // com ela: médios abaixo de 1,5 cent, graves e agudos extremos (onde
        // a cascata de 1ª ordem e o atraso de uma volta limitam) até ~6
        let (mut pior_rms, mut pior) = (0f32, 0f32);
        for k in 0..88 {
            let e = erros_parciais(k);
            let rms = (e.iter().map(|x| x * x).sum::<f32>() / e.len() as f32).sqrt();
            pior_rms = pior_rms.max(rms);
            pior = e.iter().fold(pior, |m, x| m.max(x.abs()));
        }
        println!("pior RMS {pior_rms:.2} cents, pior parcial {pior:.2} cents");
        assert!(pior_rms < 7.0 && pior < 20.0, "rms {pior_rms}, pior {pior}");
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
