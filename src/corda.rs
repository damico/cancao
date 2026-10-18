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

//! Modelo físico das 88 cordas de um piano.
//!
//! A frequência fundamental de uma corda ideal é
//!
//! ```text
//! f = (1 / 2L) * √(T / μ)
//! ```
//!
//! `L` = comprimento vibrante (m), `T` = tensão (N), `μ` = massa por
//! unidade de comprimento (kg/m). Uma corda real de aço tem rigidez, o que
//! torna os parciais levemente "esticados" (inarmonicidade):
//!
//! ```text
//! f_n = n * f * √(1 + B n²),   B = π³ E d⁴ / (64 T L²)
//! ```
//!
//! O projetista escolhe L e μ (comprimento e bitola/enrolamento); a tensão T
//! é o que o afinador ajusta para chegar na nota. Por isso aqui T é obtida
//! invertendo a fórmula, e depois f é recalculada pela fórmula da corda
//! ideal; é esse f que o sintetizador usa.

use std::f64::consts::PI;

/// A0 (tecla 0) .. C8 (tecla 87)
pub const NUM_TECLAS: usize = 88;

const RHO_ACO: f64 = 7850.0; // densidade do aço (kg/m³)
const E_ACO: f64 = 2.0e11; // módulo de Young do aço (Pa)

const F_C8: f64 = 4186.009;
const F_A0: f64 = 27.5;
const F_QUEBRA: f64 = 168.0; // abaixo disso o comprimento deixa de crescer ~1/f
const F_ENROL: f64 = 110.0; // abaixo disso as cordas são enroladas

const NOMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

#[derive(Debug, Clone)]
pub struct Corda {
    pub tecla: usize,
    pub midi: u8,
    pub nome: String,
    /// temperamento igual, A4 = 440 Hz
    pub f_alvo: f64,
    /// comprimento vibrante (m)
    pub l: f64,
    /// massa linear (kg/m)
    pub mu: f64,
    /// tensão (N)
    pub t: f64,
    /// diâmetro do fio de aço (m)
    pub d_nucleo: f64,
    /// corda de baixo com enrolamento de cobre
    pub enrolada: bool,
    /// cordas por nota (1, 2 ou 3)
    pub n_cordas: usize,
    /// fundamental pela fórmula da corda ideal
    pub f: f64,
    /// coeficiente de inarmonicidade
    pub b: f64,
}

impl Corda {
    /// Frequência do n-ésimo parcial, com inarmonicidade.
    pub fn parcial(&self, n: u32) -> f64 {
        let n = n as f64;
        n * self.f * (1.0 + self.b * n * n).sqrt()
    }
}

/// f = 1/(2L) * √(T/μ)
pub fn frequencia(l: f64, t: f64, mu: f64) -> f64 {
    (1.0 / (2.0 * l)) * (t / mu).sqrt()
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// posição logarítmica de f entre f0 e f1 (0 em f0, 1 em f1)
fn pos_log(f: f64, f0: f64, f1: f64) -> f64 {
    (f / f0).ln() / (f1 / f0).ln()
}

fn comprimento(f: f64) -> f64 {
    // agudos: L cresce quase como 1/f (C8 ~ 5 cm)
    let l_agudo = 0.052 * (F_C8 / f).powf(0.92);
    if f >= F_QUEBRA {
        return l_agudo;
    }
    // graves: o piano não comporta 5 m de corda; o comprimento cresce
    // devagar até ~1,95 m e a massa extra vem do enrolamento
    let l_q = 0.052 * (F_C8 / F_QUEBRA).powf(0.92);
    lerp(l_q, 1.95, pos_log(f, F_QUEBRA, F_A0))
}

/// Monta as 88 cordas. `fator_tensao` multiplica a tensão de todas
/// (1.0 = afinado; 1.02 = 2% mais tensas, ~ +17 cents).
pub fn cordas(fator_tensao: f64) -> Vec<Corda> {
    (0..NUM_TECLAS)
        .map(|k| {
            let midi = (k + 21) as u8;
            let nome = nome(k);
            let f_alvo = 440.0 * 2f64.powf((k as f64 - 48.0) / 12.0);
            let l = comprimento(f_alvo);

            let (d_nucleo, mu, enrolada, t) = if f_alvo >= F_ENROL {
                // corda lisa de aço: 0,775 mm no C8 até 1,05 mm nos médios-graves
                let p = pos_log(f_alvo, F_C8, F_QUEBRA).min(1.0);
                let d = lerp(0.775e-3, 1.05e-3, p);
                let mu = RHO_ACO * PI * d * d / 4.0;
                // tensão que afina a corda: T = (2 L f)² μ
                (d, mu, false, (2.0 * l * f_alvo).powi(2) * mu)
            } else {
                // corda enrolada: o fabricante escolhe a massa do cobre para
                // chegar a uma tensão de projeto (~800 N a 1600 N)
                let p = pos_log(f_alvo, F_ENROL, F_A0);
                let t = lerp(800.0, 1600.0, p);
                (lerp(1.1e-3, 1.5e-3, p), t / (2.0 * l * f_alvo).powi(2), true, t)
            };

            let t = t * fator_tensao;
            let n_cordas = if k < 8 { 1 } else if f_alvo < F_ENROL { 2 } else { 3 };
            Corda {
                tecla: k,
                midi,
                nome,
                f_alvo,
                l,
                mu,
                t,
                d_nucleo,
                enrolada,
                n_cordas,
                f: frequencia(l, t, mu),
                // a rigidez vem só do núcleo de aço
                b: PI.powi(3) * E_ACO * d_nucleo.powi(4) / (64.0 * t * l * l),
            }
        })
        .collect()
}

/// Nome da tecla 0..87: "A0", "C#4", ...
pub fn nome(tecla: usize) -> String {
    let midi = tecla + 21;
    format!("{}{}", NOMES[midi % 12], midi as i32 / 12 - 1)
}

/// Converte "A4", "C#3", "Bb2" na tecla 0..87.
pub fn tecla_de_nome(nome: &str) -> Option<usize> {
    const BASE: [i32; 7] = [9, 11, 0, 2, 4, 5, 7]; // A B C D E F G
    let mut cs = nome.chars();
    let letra = cs.next()?.to_ascii_uppercase();
    if !('A'..='G').contains(&letra) {
        return None;
    }
    let mut semitom = BASE[(letra as u8 - b'A') as usize];
    let resto = cs.as_str();
    let resto = if let Some(r) = resto.strip_prefix('#') {
        semitom += 1;
        r
    } else if let Some(r) = resto.strip_prefix('b') {
        semitom -= 1;
        r
    } else {
        resto
    };
    let oitava: i32 = resto.parse().ok()?;
    let k = (oitava + 1) * 12 + semitom - 21;
    (0..NUM_TECLAS as i32).contains(&k).then_some(k as usize)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn formula_afina_as_88_teclas() {
        for c in cordas(1.0) {
            let cents = 1200.0 * (c.f / c.f_alvo).log2();
            assert!(cents.abs() < 1e-6, "{} desafinada {cents} cents", c.nome);
        }
    }

    #[test]
    fn tensoes_e_inarmonicidade_plausiveis() {
        for c in cordas(1.0) {
            assert!((400.0..=1700.0).contains(&c.t), "{}: T = {} N", c.nome, c.t);
            assert!((5e-5..0.03).contains(&c.b), "{}: B = {}", c.nome, c.b);
        }
    }

    #[test]
    fn tensao_maior_sobe_a_afinacao() {
        // f ∝ √T: 2% de tensão a mais = +17,1 cents
        let a4 = &cordas(1.02)[48];
        let cents = 1200.0 * (a4.f / a4.f_alvo).log2();
        assert!((cents - 17.15).abs() < 0.05, "{cents}");
    }

    #[test]
    fn nomes_das_notas() {
        assert_eq!(tecla_de_nome("A0"), Some(0));
        assert_eq!(tecla_de_nome("A4"), Some(48));
        assert_eq!(tecla_de_nome("C#4"), Some(40));
        assert_eq!(tecla_de_nome("Db4"), Some(40));
        assert_eq!(tecla_de_nome("C8"), Some(87));
        assert_eq!(tecla_de_nome("C9"), None);
        assert_eq!(tecla_de_nome("H2"), None);
        assert_eq!(cordas(1.0)[40].nome, "C#4");
    }
}
