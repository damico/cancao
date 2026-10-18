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

//! Partitura: o que está escrito, sem nada de como tocar.
//!
//! É o formato em que o MusicXML (simplificado) vai ser lido: tempos em
//! semínimas a partir do início, as duas pautas do piano, dinâmica, mudanças
//! de andamento e marcações de pedal. O player decide mãos, dedos, tempo real
//! e força a partir daqui.

use crate::corda::tecla_de_nome;

/// Pauta do sistema de piano: a de cima (normalmente mão direita) e a de baixo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Pauta {
    #[default]
    Superior,
    Inferior,
}

/// Articulações escritas sobre a nota.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Articulacao {
    pub staccato: bool,
    pub staccatissimo: bool,
    pub acento: bool,
    pub tenuto: bool,
    pub fermata: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Nota {
    /// início, em semínimas desde o começo da peça
    pub inicio: f64,
    /// duração escrita, em semínimas (ligaduras de prolongamento já somadas)
    pub duracao: f64,
    /// tecla 0..87 (A0..C8)
    pub tecla: u8,
    pub pauta: Pauta,
    /// dinâmica escrita, 0..1 (pp ~0,2, mf ~0,55, ff ~0,9)
    pub dinamica: f32,
    /// voz musical (contraponto); no MusicXML de piano, 1 a 4 na pauta de
    /// cima e 5 a 8 na de baixo
    pub voz: u8,
    pub articulacao: Articulacao,
    /// dedo escrito na partitura (1 = polegar ... 5 = mínimo)
    pub dedo: Option<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Partitura {
    pub titulo: String,
    pub notas: Vec<Nota>,
    /// (a partir de qual semínima, semínimas por minuto); vazio = 120
    pub andamentos: Vec<(f64, f64)>,
    /// pedal de sustain: (abaixa, solta), em semínimas
    pub pedais: Vec<(f64, f64)>,
    /// pedal sostenuto (do meio): (abaixa, solta), em semínimas
    pub sostenuto: Vec<(f64, f64)>,
    /// una corda (esquerdo): (abaixa, solta), em semínimas
    pub una_corda: Vec<(f64, f64)>,
    /// fórmulas de compasso: (a partir de qual semínima, tempos, unidade);
    /// vazio = 4/4
    pub compassos: Vec<(f64, u32, u32)>,
    /// início (semínimas) de cada compasso como está no arquivo, inclusive
    /// anacruse e compassos irregulares; vazio = calcular pela fórmula
    pub inicios_compasso: Vec<f64>,
}

impl Partitura {
    /// Mudanças de andamento em ordem, começando em 0.
    fn marcas_de_andamento(&self) -> Vec<(f64, f64)> {
        let mut marcas = self.andamentos.clone();
        marcas.sort_by(|a, b| a.0.total_cmp(&b.0));
        if marcas.first().is_none_or(|m| m.0 > 0.0) {
            let bpm = marcas.first().map_or(120.0, |m| m.1);
            marcas.insert(0, (0.0, bpm));
        }
        marcas
    }

    /// Instante em segundos de uma posição em semínimas, pelo andamento.
    pub fn segundos(&self, tempo: f64) -> f64 {
        let marcas = self.marcas_de_andamento();
        let mut s = 0.0;
        for (i, &(ini, bpm)) in marcas.iter().enumerate() {
            let fim = marcas.get(i + 1).map_or(f64::INFINITY, |m| m.0);
            if tempo <= fim {
                return s + (tempo - ini) * 60.0 / bpm;
            }
            s += (fim - ini) * 60.0 / bpm;
        }
        s
    }

    /// O inverso de [`Partitura::segundos`]: posição em semínimas de um instante.
    pub fn tempo_em(&self, segundos: f64) -> f64 {
        let marcas = self.marcas_de_andamento();
        let mut s = 0.0;
        for (i, &(ini, bpm)) in marcas.iter().enumerate() {
            let fim = marcas.get(i + 1).map_or(f64::INFINITY, |m| m.0);
            let duracao = (fim - ini) * 60.0 / bpm;
            if segundos <= s + duracao {
                return ini + (segundos - s) * bpm / 60.0;
            }
            s += duracao;
        }
        0.0
    }

    /// Andamento (semínimas por minuto) numa posição.
    pub fn bpm_em(&self, tempo: f64) -> f64 {
        self.marcas_de_andamento().iter().rev().find(|m| m.0 <= tempo).map_or(120.0, |m| m.1)
    }

    /// Fim da última nota, em semínimas.
    pub fn fim(&self) -> f64 {
        self.notas.iter().map(|n| n.inicio + n.duracao).fold(0.0, f64::max)
    }

    /// Fórmula de compasso (tempos, unidade) numa posição.
    pub fn formula_em(&self, tempo: f64) -> (u32, u32) {
        self.compassos.iter().rev().find(|c| c.0 <= tempo + 1e-9).map_or((4, 4), |c| (c.1, c.2))
    }

    /// Início (semínimas) de cada compasso até `ate`, numerados a partir de 1.
    /// Usa os inícios do arquivo e, depois do último deles, a fórmula.
    pub fn inicios_de_compasso(&self, ate: f64) -> Vec<f64> {
        let mut inicios: Vec<f64> = self.inicios_compasso.iter().copied().take_while(|&t| t <= ate + 1e-9).collect();
        if inicios.len() < self.inicios_compasso.len() {
            return inicios;
        }
        let mut t = inicios.pop().unwrap_or(0.0);
        while t <= ate + 1e-9 && inicios.len() < 100_000 {
            inicios.push(t);
            let (tempos, unidade) = self.formula_em(t);
            t += tempos as f64 * 4.0 / unidade as f64;
        }
        inicios
    }

    /// Compasso (a partir de 1) e tempo dentro dele (a partir de 1) de uma posição.
    pub fn compasso_e_tempo(&self, tempo: f64) -> (usize, f64) {
        let inicios = self.inicios_de_compasso(tempo);
        let i = inicios.len().max(1) - 1;
        let (_, unidade) = self.formula_em(inicios[i]);
        (i + 1, 1.0 + (tempo - inicios[i]) * unidade as f64 / 4.0)
    }
}

/// Partituras de exemplo, enquanto o MusicXML não chega.
pub const EXEMPLOS: [&str; 3] = ["escala", "acordes", "desafios"];

pub fn exemplo(nome: &str) -> Option<Partitura> {
    match nome {
        "escala" => Some(escala()),
        "acordes" => Some(acordes()),
        "desafios" => Some(desafios()),
        _ => None,
    }
}

fn tecla(nome: &str) -> u8 {
    tecla_de_nome(nome).expect("nota válida") as u8
}

fn nota(inicio: f64, duracao: f64, nome: &str, pauta: Pauta, dinamica: f32) -> Nota {
    let voz = if pauta == Pauta::Superior { 1 } else { 5 };
    Nota { inicio, duracao, tecla: tecla(nome), pauta, dinamica, voz, ..Nota::default() }
}

/// Escala de Dó maior em duas oitavas, mãos paralelas, subindo e descendo.
fn escala() -> Partitura {
    let subida = ["C", "D", "E", "F", "G", "A", "B"];
    let mut nomes: Vec<(&str, i32)> = Vec::new();
    for oitava in 0..2 {
        nomes.extend(subida.iter().map(|n| (*n, oitava)));
    }
    nomes.push(("C", 2));
    let volta: Vec<(&str, i32)> = nomes.iter().rev().skip(1).copied().collect();
    nomes.extend(volta);

    let mut notas = Vec::new();
    for (i, (n, oitava)) in nomes.iter().enumerate() {
        let t = i as f64 * 0.5; // colcheias
        let ultima = i == nomes.len() - 1;
        let dur = if ultima { 2.0 } else { 0.5 };
        notas.push(nota(t, dur, &format!("{n}{}", 4 + oitava), Pauta::Superior, 0.55));
        notas.push(nota(t, dur, &format!("{n}{}", 3 + oitava), Pauta::Inferior, 0.5));
    }
    Partitura { titulo: "Escala de Dó maior, duas oitavas".into(), notas, andamentos: vec![(0.0, 100.0)], compassos: vec![(0.0, 4, 4)], ..Partitura::default() }
}

/// I - vi - IV - V - I em Dó, acordes na direita, baixo em oitavas na
/// esquerda, pedal trocado a cada acorde e um crescendo.
fn acordes() -> Partitura {
    let progressao: [(&[&str], &[&str]); 5] = [
        (&["E4", "G4", "C5"], &["C2", "C3"]),
        (&["E4", "A4", "C5"], &["A1", "A2"]),
        (&["F4", "A4", "C5"], &["F1", "F2"]),
        (&["D4", "G4", "B4"], &["G1", "G2"]),
        (&["E4", "G4", "C5"], &["C2", "C3"]),
    ];
    let mut notas = Vec::new();
    let mut pedais = Vec::new();
    for (i, (direita, esquerda)) in progressao.iter().enumerate() {
        let t = i as f64 * 2.0;
        let din = 0.4 + 0.1 * i as f32;
        let dur = if i == progressao.len() - 1 { 4.0 } else { 2.0 };
        for n in direita.iter() {
            notas.push(nota(t, dur, n, Pauta::Superior, din));
        }
        for n in esquerda.iter() {
            notas.push(nota(t, dur, n, Pauta::Inferior, din));
        }
        pedais.push((t, t + dur));
    }
    Partitura { titulo: "Cadência I-vi-IV-V-I em Dó".into(), notas, andamentos: vec![(0.0, 72.0)], pedais, compassos: vec![(0.0, 4, 4)], ..Partitura::default() }
}

/// Situações que o corpo limita: um acorde maior que a mão (arpejado) e seis
/// notas na pauta de cima ao mesmo tempo (uma passa para a mão esquerda).
fn desafios() -> Partitura {
    let mut notas = Vec::new();
    // décima: C4 a E5 não cabe numa mão média
    for n in ["C4", "E5"] {
        notas.push(nota(0.0, 2.0, n, Pauta::Superior, 0.6));
    }
    notas.push(nota(0.0, 2.0, "C3", Pauta::Inferior, 0.5));
    // seis notas na pauta de cima, com a mão esquerda livre
    for n in ["G3", "C4", "E4", "G4", "B4", "D5"] {
        notas.push(nota(2.0, 2.0, n, Pauta::Superior, 0.6));
    }
    Partitura { titulo: "Desafios: acorde largo e seis notas numa pauta".into(), notas, andamentos: vec![(0.0, 60.0)], compassos: vec![(0.0, 4, 4)], ..Partitura::default() }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn andamento_converte_seminimas_em_segundos() {
        let mut p = Partitura::default();
        assert_eq!(p.segundos(4.0), 2.0); // 120 por padrão
        p.andamentos = vec![(0.0, 60.0), (4.0, 120.0)];
        assert_eq!(p.segundos(4.0), 4.0);
        assert_eq!(p.segundos(6.0), 5.0);
    }

    #[test]
    fn tempo_em_desfaz_segundos() {
        let p = Partitura { andamentos: vec![(0.0, 60.0), (4.0, 120.0), (10.0, 90.0)], ..Partitura::default() };
        for t in [0.0, 1.5, 4.0, 7.25, 10.0, 13.0] {
            assert!((p.tempo_em(p.segundos(t)) - t).abs() < 1e-9, "{t}");
        }
        assert_eq!(p.bpm_em(5.0), 120.0);
    }

    #[test]
    fn compassos() {
        let mut p = Partitura::default();
        assert_eq!(p.compasso_e_tempo(5.5), (2, 2.5)); // 4/4
        // 3/4 e depois 6/8 a partir da semínima 6
        p.compassos = vec![(0.0, 3, 4), (6.0, 6, 8)];
        assert_eq!(p.inicios_de_compasso(10.0), [0.0, 3.0, 6.0, 9.0]);
        assert_eq!(p.compasso_e_tempo(7.0), (3, 3.0)); // 6/8: a colcheia é o tempo
    }

    #[test]
    fn exemplos_existem() {
        for nome in EXEMPLOS {
            let p = exemplo(nome).unwrap();
            assert!(!p.notas.is_empty(), "{nome}");
        }
        assert!(exemplo("nada").is_none());
    }
}
