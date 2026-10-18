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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pauta {
    Superior,
    Inferior,
}

#[derive(Debug, Clone, PartialEq)]
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
}

#[derive(Debug, Clone, Default)]
pub struct Partitura {
    pub titulo: String,
    pub notas: Vec<Nota>,
    /// (a partir de qual semínima, semínimas por minuto); vazio = 120
    pub andamentos: Vec<(f64, f64)>,
    /// pedal de sustain: (abaixa, solta), em semínimas
    pub pedais: Vec<(f64, f64)>,
}

impl Partitura {
    /// Instante em segundos de uma posição em semínimas, pelo andamento.
    pub fn segundos(&self, tempo: f64) -> f64 {
        let mut marcas = self.andamentos.clone();
        marcas.sort_by(|a, b| a.0.total_cmp(&b.0));
        if marcas.first().is_none_or(|m| m.0 > 0.0) {
            let bpm = marcas.first().map_or(120.0, |m| m.1);
            marcas.insert(0, (0.0, bpm));
        }
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
    Nota { inicio, duracao, tecla: tecla(nome), pauta, dinamica }
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
    Partitura { titulo: "Escala de Dó maior, duas oitavas".into(), notas, andamentos: vec![(0.0, 100.0)], pedais: vec![] }
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
    Partitura { titulo: "Cadência I-vi-IV-V-I em Dó".into(), notas, andamentos: vec![(0.0, 72.0)], pedais }
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
    Partitura { titulo: "Desafios: acorde largo e seis notas numa pauta".into(), notas, andamentos: vec![(0.0, 60.0)], pedais: vec![] }
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
    fn exemplos_existem() {
        for nome in EXEMPLOS {
            let p = exemplo(nome).unwrap();
            assert!(!p.notas.is_empty(), "{nome}");
        }
        assert!(exemplo("nada").is_none());
    }
}
