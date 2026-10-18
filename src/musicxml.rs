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

//! Leitor simplificado de MusicXML 4.0 para uma [`Partitura`].
//!
//! Entende:
//!
//! - `score-partwise` não comprimido (`.musicxml` / `.xml`);
//! - `divisions`, `note` com `pitch` (`step`, `alter`, `octave`) e
//!   `duration`, pausas, `chord`, `backup` e `forward`;
//! - `staff`: 1 = pauta de cima (mão direita), 2 = pauta de baixo; sem
//!   `staff`, a 1ª parte vai para cima e as outras para baixo;
//! - ligaduras de prolongamento (`tie` start/stop): as notas viram uma só;
//! - andamento (`sound tempo`), dinâmicas (`pppp` a `ffff`) e pedal de
//!   sustain (`pedal` start/stop/change).
//!
//! Ignora (sem erro): `voice`, `grace`, ornamentos, articulações, repetições,
//! `offset` de direções, letras e tudo de diagramação. Não lê `score-timewise`
//! nem o formato comprimido `.mxl`.

use std::collections::HashMap;
use std::path::Path;

use roxmltree::{Document, Node, ParsingOptions};

use crate::corda::NUM_TECLAS;
use crate::partitura::{Nota, Partitura, Pauta};

pub fn ler(caminho: &Path) -> Result<Partitura, String> {
    let texto = std::fs::read_to_string(caminho).map_err(|e| format!("{}: {e}", caminho.display()))?;
    de_texto(&texto).map_err(|e| format!("{}: {e}", caminho.display()))
}

/// Filho com este nome.
fn filho<'a, 'i>(n: Node<'a, 'i>, nome: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(nome))
}

fn texto_de(n: Node, nome: &str) -> Option<String> {
    filho(n, nome).and_then(|c| c.text()).map(|t| t.trim().to_string())
}

fn numero(n: Node, nome: &str) -> Option<f64> {
    texto_de(n, nome).and_then(|t| t.parse().ok())
}

/// Dinâmica escrita para a força 0..1.
fn dinamica(nome: &str) -> Option<f32> {
    Some(match nome {
        "pppp" => 0.10,
        "ppp" => 0.15,
        "pp" => 0.25,
        "p" => 0.35,
        "mp" => 0.45,
        "mf" => 0.55,
        "f" => 0.70,
        "ff" => 0.85,
        "fff" => 0.95,
        "ffff" => 1.0,
        _ => return None,
    })
}

pub fn de_texto(xml: &str) -> Result<Partitura, String> {
    // o cabeçalho padrão traz um DOCTYPE; a DTD não é baixada nem usada
    let opcoes = ParsingOptions { allow_dtd: true, ..ParsingOptions::default() };
    let doc = Document::parse_with_options(xml, opcoes).map_err(|e| format!("XML inválido: {e}"))?;
    let raiz = doc.root_element();
    if !raiz.has_tag_name("score-partwise") {
        return Err(format!("esperado score-partwise, veio {}", raiz.tag_name().name()));
    }

    let titulo = filho(raiz, "work")
        .and_then(|w| texto_de(w, "work-title"))
        .or_else(|| texto_de(raiz, "movement-title"))
        .unwrap_or_else(|| "(sem título)".into());
    let mut p = Partitura { titulo, ..Partitura::default() };

    for (i_parte, parte) in raiz.children().filter(|c| c.has_tag_name("part")).enumerate() {
        let pauta_padrao = if i_parte == 0 { Pauta::Superior } else { Pauta::Inferior };
        // posições em semínimas
        let mut divisoes = 1.0;
        let mut cursor = 0.0;
        let mut inicio_ultima = 0.0;
        let mut forca = 0.55; // mf até aparecer outra dinâmica
        let mut pedal_desde: Option<f64> = None;
        // ligadura aberta: (tecla, pauta) -> índice da nota
        let mut ligadas: HashMap<(u8, Pauta), usize> = HashMap::new();

        for compasso in parte.children().filter(|c| c.has_tag_name("measure")) {
            for el in compasso.children().filter(|c| c.is_element()) {
                match el.tag_name().name() {
                    "attributes" => {
                        if let Some(d) = numero(el, "divisions") {
                            divisoes = d;
                        }
                    }
                    "backup" => cursor -= numero(el, "duration").unwrap_or(0.0) / divisoes,
                    "forward" => cursor += numero(el, "duration").unwrap_or(0.0) / divisoes,
                    "sound" => {
                        if let Some(bpm) = el.attribute("tempo").and_then(|t| t.parse().ok()) {
                            p.andamentos.push((cursor, bpm));
                        }
                    }
                    "direction" => {
                        if let Some(bpm) = filho(el, "sound").and_then(|s| s.attribute("tempo")).and_then(|t| t.parse().ok()) {
                            p.andamentos.push((cursor, bpm));
                        }
                        for tipo in el.children().filter(|c| c.has_tag_name("direction-type")) {
                            if let Some(d) = filho(tipo, "dynamics") {
                                if let Some(v) = d.children().filter(|c| c.is_element()).find_map(|c| dinamica(c.tag_name().name())) {
                                    forca = v;
                                }
                            }
                            if let Some(ped) = filho(tipo, "pedal") {
                                let fecha = |desde: Option<f64>, p: &mut Partitura| {
                                    if let Some(d) = desde {
                                        p.pedais.push((d, cursor));
                                    }
                                };
                                match ped.attribute("type") {
                                    Some("start") => pedal_desde = Some(cursor),
                                    Some("stop") => {
                                        fecha(pedal_desde, &mut p);
                                        pedal_desde = None;
                                    }
                                    Some("change") => {
                                        fecha(pedal_desde, &mut p);
                                        pedal_desde = Some(cursor);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    "note" => {
                        if filho(el, "grace").is_some() {
                            continue; // ornamento sem duração
                        }
                        let dur = numero(el, "duration").unwrap_or(0.0) / divisoes;
                        let acorde = filho(el, "chord").is_some();
                        let inicio = if acorde { inicio_ultima } else { cursor };
                        if !acorde {
                            inicio_ultima = cursor;
                            cursor += dur;
                        }
                        let Some(altura) = filho(el, "pitch") else { continue }; // pausa
                        let passo = match texto_de(altura, "step").as_deref() {
                            Some("C") => 0,
                            Some("D") => 2,
                            Some("E") => 4,
                            Some("F") => 5,
                            Some("G") => 7,
                            Some("A") => 9,
                            Some("B") => 11,
                            outro => return Err(format!("step inválido: {outro:?}")),
                        };
                        let alter = numero(altura, "alter").unwrap_or(0.0).round() as i32;
                        let oitava = numero(altura, "octave").ok_or("nota sem octave")? as i32;
                        let tecla = (oitava + 1) * 12 + passo + alter - 21;
                        if !(0..NUM_TECLAS as i32).contains(&tecla) {
                            continue; // fora das 88 teclas
                        }
                        let tecla = tecla as u8;
                        let pauta = match texto_de(el, "staff").as_deref() {
                            Some("1") => Pauta::Superior,
                            Some(_) => Pauta::Inferior,
                            None => pauta_padrao,
                        };

                        let ties: Vec<&str> = el.children().filter(|c| c.has_tag_name("tie")).filter_map(|c| c.attribute("type")).collect();
                        // continuação de uma ligadura: estende a nota anterior
                        if ties.contains(&"stop") {
                            if let Some(&i) = ligadas.get(&(tecla, pauta)) {
                                p.notas[i].duracao += dur;
                                if !ties.contains(&"start") {
                                    ligadas.remove(&(tecla, pauta));
                                }
                                continue;
                            }
                        }
                        p.notas.push(Nota { inicio, duracao: dur, tecla, pauta, dinamica: forca });
                        if ties.contains(&"start") {
                            ligadas.insert((tecla, pauta), p.notas.len() - 1);
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(d) = pedal_desde {
            p.pedais.push((d, cursor)); // pedal não solto até o fim
        }
    }
    p.notas.sort_by(|a, b| a.inicio.total_cmp(&b.inicio).then(a.tecla.cmp(&b.tecla)));
    p.andamentos.sort_by(|a, b| a.0.total_cmp(&b.0));
    p.andamentos.dedup_by(|a, b| a.0 == b.0);
    Ok(p)
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::corda::tecla_de_nome;

    const ODE: &str = include_str!("../exemplos/ode.musicxml");

    fn t(nome: &str) -> u8 {
        tecla_de_nome(nome).unwrap() as u8
    }

    #[test]
    fn le_o_exemplo() {
        let p = de_texto(ODE).unwrap();
        assert_eq!(p.titulo, "Ode à Alegria (tema)");
        // 61 elementos note, um deles é continuação de ligadura
        assert_eq!(p.notas.len(), 60);
        assert_eq!(p.notas.iter().filter(|n| n.pauta == Pauta::Superior).count(), 30);
        assert_eq!(p.andamentos, [(0.0, 100.0)]);
        // pedal: abaixa no 1º compasso, troca em cada um dos 8 seguintes
        assert_eq!(p.pedais.len(), 9);
        assert_eq!(p.pedais[0], (0.0, 4.0));
        assert_eq!(p.pedais[8], (32.0, 36.0));
    }

    #[test]
    fn acordes_backup_e_ligadura() {
        let p = de_texto(ODE).unwrap();
        // 1º compasso: o acorde da pauta de baixo começa junto com a melodia
        let no_inicio: Vec<u8> = p.notas.iter().filter(|n| n.inicio == 0.0).map(|n| n.tecla).collect();
        assert_eq!(no_inicio, [t("C3"), t("E3"), t("G3"), t("E4")]);
        // compasso 8: o C4 de mínima ligado à semibreve do compasso 9 vira 6 tempos
        let ligada = p.notas.iter().find(|n| n.tecla == t("C4") && n.inicio == 30.0).unwrap();
        assert_eq!(ligada.duracao, 6.0);
        // colcheia depois da semínima pontuada
        assert!(p.notas.iter().any(|n| n.tecla == t("D4") && n.inicio == 13.5 && n.duracao == 0.5));
    }

    #[test]
    fn dinamicas() {
        let p = de_texto(ODE).unwrap();
        let forca_em = |tempo: f64| p.notas.iter().find(|n| n.inicio == tempo && n.pauta == Pauta::Superior).unwrap().dinamica;
        assert_eq!(forca_em(0.0), 0.55); // mf
        assert_eq!(forca_em(16.0), 0.35); // p no compasso 5
        assert_eq!(forca_em(24.0), 0.70); // f no compasso 7
    }

    #[test]
    fn erros_claros() {
        assert!(de_texto("<score-timewise/>").unwrap_err().contains("score-partwise"));
        assert!(de_texto("<score-partwise>").unwrap_err().contains("XML inválido"));
    }

    #[test]
    fn alteracoes_e_pausas() {
        let xml = r#"<score-partwise version="4.0"><part id="P1"><measure number="1">
            <attributes><divisions>1</divisions></attributes>
            <note><rest/><duration>1</duration></note>
            <note><pitch><step>F</step><alter>1</alter><octave>4</octave></pitch><duration>1</duration></note>
            <note><pitch><step>B</step><alter>-1</alter><octave>3</octave></pitch><duration>2</duration></note>
        </measure></part></score-partwise>"#;
        let p = de_texto(xml).unwrap();
        assert_eq!(p.notas.len(), 2);
        assert_eq!((p.notas[0].tecla, p.notas[0].inicio), (t("F#4"), 1.0));
        assert_eq!((p.notas[1].tecla, p.notas[1].inicio), (t("Bb3"), 2.0));
        assert_eq!(p.notas[0].pauta, Pauta::Superior);
    }
}
