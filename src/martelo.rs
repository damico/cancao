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

//! Martelo de feltro: uma massa lançada contra a corda, com o feltro como
//! mola não linear,
//!
//! ```text
//! F = K * compressão^p        (p entre 2,3 e 3: endurece ao comprimir)
//! ```
//!
//! A corda resiste com a impedância Z = √(T μ) de cada lado do ponto de
//! contato, e as ondas que voltam das pontas (agrafe e cavalete) empurram o
//! martelo de volta. Daí sai sozinho:
//!
//! - golpe forte: o feltro comprime mais, fica mais duro, o contato fica mais
//!   curto e a força mais pontuda; o som fica mais brilhante;
//! - contato de ~4 ms nos graves a menos de 1 ms nos agudos;
//! - nos agudos o contato dura várias voltas da corda, e as reflexões mexem
//!   no formato do golpe.
//!
//! Massa, K e p dos martelos: medidas de Chaigne e Askenfelt (1994) para
//! C2, C4 e C7, interpoladas em log da frequência.

use crate::corda::Corda;
use crate::synth::POS_MARTELO;

/// Subpassos por amostra: a mola do feltro é dura e precisa de passo curto.
const SUB: usize = 4;
/// Maior golpe que cabe (amostras): ~10 ms a 48 kHz.
pub const MAX_GOLPE: usize = 512;
/// Histórico das ondas no ponto de contato, em subpassos (uma volta do A0 a
/// 48 kHz com 4 subpassos tem ~7000).
const MAX_HIST: usize = 8192;

/// (frequência, massa kg, p, ln K) medidos em C2, C4 e C7
const MARTELOS: [(f32, f32, f32, f32); 3] = [
    (65.41, 4.90e-3, 2.3, 19.807), // K = 4,0e8
    (261.63, 2.97e-3, 2.5, 22.227), // K = 4,5e9
    (2093.0, 2.20e-3, 3.0, 27.631), // K = 1,0e12
];

/// Interpola uma coluna de MARTELOS em log da frequência. Abaixo do C2
/// extrapola (martelos de grave são mais pesados e macios); acima do C7 fica
/// no C7: extrapolar a dureza do feltro daria um K centenas de vezes maior
/// no C8, sem base nas medidas, e um golpe que é praticamente um impulso.
fn interpolar(f: f32, coluna: impl Fn(&(f32, f32, f32, f32)) -> f32) -> f32 {
    let i = if f < MARTELOS[1].0 { 0 } else { 1 };
    let (a, b) = (&MARTELOS[i], &MARTELOS[i + 1]);
    let t = ((f / a.0).ln() / (b.0 / a.0).ln()).min(1.0);
    coluna(a) + (coluna(b) - coluna(a)) * t
}

/// Velocidade do martelo (m/s) para a força da tecla 0..1: pp ~0,5, ff ~5.
pub fn velocidade(vel: f32) -> f32 {
    0.5 + 4.5 * vel * vel
}

/// Simula o golpe e escreve a força na corda (N) em `forca`, uma por
/// amostra. Devolve quantas amostras o golpe durou.
///
/// `atingidas`: cordas do uníssono que o martelo acerta (todas recebem a
/// mesma força). `una_corda`: posição do pedal (feltro mais macio).
pub fn golpe(c: &Corda, atingidas: usize, vel: f32, una_corda: f32, taxa: f32, forca: &mut [f32; MAX_GOLPE]) -> usize {
    let f = c.parcial(1) as f32;
    let massa = interpolar(f, |m| m.1);
    let p = interpolar(f, |m| m.2);
    // una corda: o martelo bate com uma parte do feltro menos compactada
    let k = interpolar(f, |m| m.3).exp() * (1.0 - 0.5 * una_corda);
    // impedância de cada lado do contato, somada nas cordas atingidas
    let z = (c.t * c.mu).sqrt() as f32 * atingidas.max(1) as f32;

    // ida e volta até a agrafe (perto) e até o cavalete (longe), em subpassos
    let volta = taxa / f * SUB as f32;
    let perto = ((POS_MARTELO * volta).round() as usize).clamp(1, MAX_HIST - 1);
    let longe = ((volta - perto as f32).round() as usize).clamp(1, MAX_HIST - 1);

    // ondas de velocidade saindo do contato: rumo à agrafe (a) e ao cavalete (b)
    let mut a = [0f32; MAX_HIST];
    let mut b = [0f32; MAX_HIST];
    let dt = 1.0 / (taxa * SUB as f32);
    let (mut y_m, mut v_m, mut y_c) = (0f32, velocidade(vel), 0f32);
    let mut tocou = false;
    forca.fill(0.0);

    let mut s = 0;
    while s < MAX_GOLPE * SUB {
        // o que volta das pontas, invertido (pontas fixas)
        let de_perto = if s >= perto { -a[(s - perto) % MAX_HIST] } else { 0.0 };
        let de_longe = if s >= longe { -b[(s - longe) % MAX_HIST] } else { 0.0 };

        let compressao = y_m - y_c;
        let f_n = if compressao > 0.0 { k * compressao.powf(p) } else { 0.0 };
        let empurrao = f_n / (2.0 * z);
        a[s % MAX_HIST] = de_longe + empurrao;
        b[s % MAX_HIST] = de_perto + empurrao;
        let v_c = de_perto + de_longe + empurrao;

        v_m -= f_n / massa * dt;
        y_m += v_m * dt;
        y_c += v_c * dt;
        forca[s / SUB] += f_n / SUB as f32;

        tocou |= f_n > 0.0;
        // o martelo soltou da corda e está voltando: acabou
        if tocou && f_n == 0.0 && v_m < v_c {
            break;
        }
        s += 1;
    }
    (s / SUB + 1).min(MAX_GOLPE)
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::corda::{self, tecla_de_nome};

    fn bater(nome: &str, vel: f32) -> (usize, f32, Vec<f32>) {
        let cordas = corda::cordas(1.0);
        let c = &cordas[tecla_de_nome(nome).unwrap()];
        let mut forca = [0f32; MAX_GOLPE];
        let n = golpe(c, c.n_cordas, vel, 0.0, 48000.0, &mut forca);
        let pico = forca.iter().fold(0f32, |m, &x| m.max(x));
        (n, pico, forca[..n].to_vec())
    }

    /// fração da energia do golpe acima de `f_corte` Hz
    fn agudos(forca: &[f32], f_corte: f32) -> f32 {
        let taxa = 48000.0;
        let (mut alto, mut total) = (0f32, 0f32);
        for k in 1..400 {
            let fk = k as f32 * 20.0;
            let w = 2.0 * std::f32::consts::PI * fk / taxa;
            let (re, im) = forca.iter().enumerate().fold((0f32, 0f32), |(re, im), (i, &x)| {
                (re + x * (w * i as f32).cos(), im - x * (w * i as f32).sin())
            });
            let e = re * re + im * im;
            total += e;
            if fk > f_corte {
                alto += e;
            }
        }
        alto / total
    }

    #[test]
    fn contato_mais_curto_nos_agudos() {
        let ms = |nome| bater(nome, 0.6).0 as f32 / 48.0;
        let (grave, medio, agudo) = (ms("A1"), ms("C4"), ms("C7"));
        println!("contato: A1 {grave:.2} ms, C4 {medio:.2} ms, C7 {agudo:.2} ms");
        assert!(grave > medio && medio > agudo, "{grave} {medio} {agudo}");
        assert!((1.5..8.0).contains(&grave) && (0.2..1.5).contains(&agudo));
    }

    #[test]
    fn golpe_forte_e_mais_curto_mais_forte_e_mais_brilhante() {
        let (n_pp, pico_pp, f_pp) = bater("C4", 0.1);
        let (n_ff, pico_ff, f_ff) = bater("C4", 1.0);
        let (b_pp, b_ff) = (agudos(&f_pp, 1500.0), agudos(&f_ff, 1500.0));
        println!("C4 pp: {n_pp} amostras, pico {pico_pp:.1} N, agudos {b_pp:.4}");
        println!("C4 ff: {n_ff} amostras, pico {pico_ff:.1} N, agudos {b_ff:.4}");
        assert!(n_ff < n_pp, "contato");
        // não linear: 10x a velocidade dá bem mais que 10x a força
        let razao_v = velocidade(1.0) / velocidade(0.1);
        assert!(pico_ff / pico_pp > razao_v, "pico {pico_pp} -> {pico_ff}");
        assert!(b_ff > 2.0 * b_pp, "brilho {b_pp} -> {b_ff}");
    }

    #[test]
    fn todas_as_teclas_dao_um_golpe_valido() {
        let cordas = corda::cordas(1.0);
        for c in &cordas {
            for vel in [0.05, 0.5, 1.0] {
                let mut forca = [0f32; MAX_GOLPE];
                let n = golpe(c, c.n_cordas, vel, 0.0, 48000.0, &mut forca);
                assert!(n > 2 && n < MAX_GOLPE, "{} vel {vel}: {n} amostras", c.nome);
                assert!(forca[..n].iter().all(|x| x.is_finite() && *x >= 0.0), "{}", c.nome);
            }
        }
    }
}
