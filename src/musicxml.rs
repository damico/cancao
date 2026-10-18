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
//! - `voice` (vozes do contraponto) e fórmula de compasso (`time`);
//! - ligaduras de prolongamento (`tie` start/stop): as notas viram uma só;
//! - andamento (`sound tempo`), dinâmicas (`pppp` a `ffff`), crescendo e
//!   diminuendo (`wedge`, interpolando até a dinâmica seguinte), pedal de
//!   sustain (`pedal` start/stop/change), sostenuto (`pedal` sostenuto, até
//!   o stop seguinte) e una corda (texto "una corda" ou "u.c." até "tre
//!   corde", "tutte le corde" ou "t.c.");
//! - articulações (staccato, staccatissimo, acento, tenuto, fermata) e
//!   dedilhado escrito (`fingering`);
//! - o formato comprimido `.mxl` (um ZIP com o MusicXML dentro).
//!
//! Ignora (sem erro): `grace`, ornamentos, repetições, `offset` de
//! direções, letras e tudo de diagramação. Não lê `score-timewise`.

use std::collections::HashMap;
use std::path::Path;

use roxmltree::{Document, Node, ParsingOptions};

use crate::corda::NUM_TECLAS;
use crate::partitura::{Nota, Partitura, Pauta};

/// O texto MusicXML de um arquivo `.musicxml`/`.xml`, ou de dentro de um
/// `.mxl` (ZIP: o arquivo principal vem indicado em META-INF/container.xml).
pub fn ler_texto(caminho: &Path) -> Result<String, String> {
    ler_inicio(caminho, u64::MAX)
}

/// Como [`ler_texto`], mas só os primeiros `limite` bytes do MusicXML (o
/// cabeçalho, para indexar sem descomprimir tudo).
pub fn ler_inicio(caminho: &Path, limite: u64) -> Result<String, String> {
    use std::io::{Read, Seek};
    let erro = |e: &dyn std::fmt::Display| format!("{}: {e}", caminho.display());
    let mut arquivo = std::fs::File::open(caminho).map_err(|e| erro(&e))?;
    let mut assinatura = [0u8; 2];
    let zipado = arquivo.read_exact(&mut assinatura).is_ok() && &assinatura == b"PK";
    arquivo.rewind().map_err(|e| erro(&e))?;
    let mut bytes = Vec::new();
    if !zipado {
        arquivo.take(limite).read_to_end(&mut bytes).map_err(|e| erro(&e))?;
        return Ok(decodificar(bytes));
    }
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(arquivo)).map_err(|e| erro(&e))?;
    let mut container = Vec::new();
    zip.by_name("META-INF/container.xml").and_then(|mut z| Ok(z.read_to_end(&mut container)?)).map_err(|e| erro(&e))?;
    let container = decodificar(container);
    let doc = Document::parse(&container).map_err(|e| erro(&e))?;
    let principal = doc
        .descendants()
        .find(|n| n.has_tag_name("rootfile"))
        .and_then(|n| n.attribute("full-path"))
        .ok_or_else(|| erro(&"MXL sem rootfile em META-INF/container.xml"))?
        .to_string();
    zip.by_name(&principal).map_err(|e| erro(&e))?.take(limite).read_to_end(&mut bytes).map_err(|e| erro(&e))?;
    Ok(decodificar(bytes))
}

/// Bytes do XML para texto: UTF-8 (o comum) ou UTF-16 com BOM; um corte no
/// meio de um caractere (em [`ler_inicio`]) vira o caractere de troca.
fn decodificar(bytes: Vec<u8>) -> String {
    let utf16 = |par: fn([u8; 2]) -> u16| {
        let unidades: Vec<u16> = bytes[2..].as_chunks::<2>().0.iter().map(|&c| par(c)).collect();
        String::from_utf16_lossy(&unidades)
    };
    match bytes.get(..2) {
        Some([0xFF, 0xFE]) => utf16(u16::from_le_bytes),
        Some([0xFE, 0xFF]) => utf16(u16::from_be_bytes),
        _ => String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
    }
}

pub fn ler(caminho: &Path) -> Result<Partitura, String> {
    let texto = ler_texto(caminho)?;
    let mut p = de_texto(&texto).map_err(|e| format!("{}: {e}", caminho.display()))?;
    if p.titulo.is_empty() {
        p.titulo = caminho.file_stem().map_or("(sem título)".into(), |n| n.to_string_lossy().into_owned());
    }
    Ok(p)
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
/// Texto de una corda: Some(true) abaixa, Some(false) solta.
fn una_corda(texto: &str) -> Option<bool> {
    let t = texto.trim().to_lowercase();
    if t.contains("una corda") || t == "u.c." {
        Some(true)
    } else if t.contains("tre corde") || t.contains("tutte le corde") || t == "t.c." {
        Some(false)
    } else {
        None
    }
}

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

/// Articulações e dedo escrito de um elemento `note`.
fn articulacoes(nota: Node) -> (crate::partitura::Articulacao, Option<u8>) {
    let mut a = crate::partitura::Articulacao::default();
    let mut dedo = None;
    for n in nota.children().filter(|c| c.has_tag_name("notations")) {
        if let Some(arts) = filho(n, "articulations") {
            for c in arts.children().filter(|c| c.is_element()) {
                match c.tag_name().name() {
                    "staccato" => a.staccato = true,
                    "staccatissimo" | "spiccato" => a.staccatissimo = true,
                    "accent" | "strong-accent" => a.acento = true,
                    "tenuto" | "detached-legato" => a.tenuto = true,
                    _ => {}
                }
            }
        }
        a.fermata |= filho(n, "fermata").is_some();
        if let Some(t) = filho(n, "technical") {
            dedo = dedo.or(texto_de(t, "fingering").and_then(|f| f.parse::<u8>().ok()).filter(|d| (1..=5).contains(d)));
        }
    }
    (a, dedo)
}

/// Crescendo/diminuendo: a força vai da dinâmica do começo da cunha até a
/// dinâmica marcada logo depois do fim dela (ou um degrau, se não houver).
fn aplicar_cunhas(notas: &mut [Nota], marcas: &[(f64, f32)], cunhas: &[(f64, f64, f32)]) {
    let em = |t: f64| marcas.iter().rev().find(|m| m.0 <= t + 1e-9).map_or(0.55, |m| m.1);
    for &(a, b, sentido) in cunhas {
        if b <= a {
            continue;
        }
        let v0 = em(a);
        // a dinâmica de chegada vem marcada até ~1 compasso depois do fim
        let chegada = marcas.iter().find(|m| m.0 >= b - 1e-9 && m.0 <= b + 4.0);
        let v1 = chegada.map_or((v0 + 0.15 * sentido).clamp(0.05, 1.0), |m| m.1);
        let ate = chegada.map_or(b, |m| m.0);
        for n in notas.iter_mut().filter(|n| n.inicio >= a - 1e-9 && n.inicio < ate) {
            let u = ((n.inicio - a) / (b - a)).min(1.0) as f32;
            n.dinamica = v0 + (v1 - v0) * u;
        }
    }
}

pub fn de_texto(xml: &str) -> Result<Partitura, String> {
    // o cabeçalho padrão traz um DOCTYPE; a DTD não é baixada nem usada
    let opcoes = ParsingOptions { allow_dtd: true, ..ParsingOptions::default() };
    let doc = Document::parse_with_options(xml, opcoes).map_err(|e| format!("XML inválido: {e}"))?;
    let raiz = doc.root_element();
    if !raiz.has_tag_name("score-partwise") {
        return Err(format!("esperado score-partwise, veio {}", raiz.tag_name().name()));
    }

    // título: da obra, do movimento ou o texto impresso como título na página
    let credito = || {
        let creditos: Vec<Node> = raiz.children().filter(|c| c.has_tag_name("credit")).collect();
        let do_tipo_titulo = creditos.iter().find(|c| texto_de(**c, "credit-type").as_deref() == Some("title"));
        do_tipo_titulo.or(creditos.first()).and_then(|c| texto_de(*c, "credit-words"))
    };
    let titulo = filho(raiz, "work")
        .and_then(|w| texto_de(w, "work-title"))
        .or_else(|| texto_de(raiz, "movement-title"))
        .or_else(credito)
        .unwrap_or_default();
    let mut p = Partitura { titulo, ..Partitura::default() };

    for (i_parte, parte) in raiz.children().filter(|c| c.has_tag_name("part")).enumerate() {
        let pauta_padrao = if i_parte == 0 { Pauta::Superior } else { Pauta::Inferior };
        // posições em semínimas
        let mut divisoes = 1.0;
        let mut cursor = 0.0;
        let mut inicio_ultima = 0.0;
        let mut forca = 0.55; // mf até aparecer outra dinâmica
        let mut pedal_desde: Option<f64> = None;
        let mut sostenuto_desde: Option<f64> = None;
        let mut una_corda_desde: Option<f64> = None;
        // para os crescendos: as dinâmicas marcadas e os trechos com cunha
        let primeira_nota = p.notas.len();
        let mut marcas: Vec<(f64, f32)> = Vec::new();
        let mut cunhas: Vec<(f64, f64, f32)> = Vec::new();
        let mut cunha_desde: Option<(f64, f32)> = None;
        // ligadura aberta: (tecla, pauta) -> índice da nota
        let mut ligadas: HashMap<(u8, Pauta), usize> = HashMap::new();

        for compasso in parte.children().filter(|c| c.has_tag_name("measure")) {
            if i_parte == 0 {
                p.inicios_compasso.push(cursor);
            }
            // o compasso termina no ponto mais à frente que alguma voz alcançou
            // (uma voz incompleta no fim não pode adiantar o compasso seguinte)
            let mut maximo = cursor;
            for el in compasso.children().filter(|c| c.is_element()) {
                maximo = f64::max(maximo, cursor);
                match el.tag_name().name() {
                    "attributes" => {
                        if let Some(d) = numero(el, "divisions") {
                            divisoes = d;
                        }
                        if let Some(t) = filho(el, "time") {
                            let tempos = texto_de(t, "beats").and_then(|b| b.parse().ok());
                            let unidade = texto_de(t, "beat-type").and_then(|b| b.parse().ok());
                            if let (Some(tempos), Some(unidade)) = (tempos, unidade) {
                                if i_parte == 0 && p.compassos.last().is_none_or(|c| c.0 < cursor) {
                                    p.compassos.push((cursor, tempos, unidade));
                                }
                            }
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
                                    marcas.push((cursor, v));
                                }
                            }
                            if let Some(w) = filho(tipo, "wedge") {
                                match w.attribute("type") {
                                    Some("crescendo") => cunha_desde = Some((cursor, 1.0)),
                                    Some("diminuendo") => cunha_desde = Some((cursor, -1.0)),
                                    Some("stop") => {
                                        if let Some((ini, sentido)) = cunha_desde.take() {
                                            cunhas.push((ini, cursor, sentido));
                                        }
                                    }
                                    _ => {}
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
                                    Some("sostenuto") => sostenuto_desde = Some(cursor),
                                    // o stop fecha o sostenuto só se o sustain não estiver abaixado
                                    Some("stop") if pedal_desde.is_none() => {
                                        if let Some(d) = sostenuto_desde.take() {
                                            p.sostenuto.push((d, cursor));
                                        }
                                    }
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
                            if let Some(w) = texto_de(tipo, "words") {
                                match una_corda(&w) {
                                    Some(true) => una_corda_desde = una_corda_desde.or(Some(cursor)),
                                    Some(false) => {
                                        if let Some(d) = una_corda_desde.take() {
                                            p.una_corda.push((d, cursor));
                                        }
                                    }
                                    None => {}
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
                        let voz = texto_de(el, "voice").and_then(|v| v.parse().ok()).unwrap_or(if pauta == Pauta::Superior { 1 } else { 5 });
                        let (articulacao, dedo) = articulacoes(el);
                        p.notas.push(Nota { inicio, duracao: dur, tecla, pauta, dinamica: forca, voz, articulacao, dedo });
                        if ties.contains(&"start") {
                            ligadas.insert((tecla, pauta), p.notas.len() - 1);
                        }
                    }
                    _ => {}
                }
            }
            cursor = f64::max(maximo, cursor);
        }
        if let Some(d) = pedal_desde {
            p.pedais.push((d, cursor)); // pedal não solto até o fim
        }
        if let Some(d) = sostenuto_desde {
            p.sostenuto.push((d, cursor));
        }
        if let Some(d) = una_corda_desde {
            p.una_corda.push((d, cursor));
        }
        aplicar_cunhas(&mut p.notas[primeira_nota..], &marcas, &cunhas);
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
        assert_eq!(p.compassos, [(0.0, 4, 4)]);
        // vozes: 1 na pauta de cima, 5 na de baixo
        assert!(p.notas.iter().all(|n| n.voz == if n.pauta == Pauta::Superior { 1 } else { 5 }));
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
    fn sostenuto_e_una_corda() {
        let nota = r#"<note><pitch><step>C</step><octave>4</octave></pitch><duration>1</duration></note>"#;
        let dir = |x: &str| format!("<direction><direction-type>{x}</direction-type></direction>");
        let xml = format!(
            r#"<score-partwise version="4.0"><part id="P1"><measure number="1">
            <attributes><divisions>1</divisions></attributes>
            {}{}{nota}{nota}{}{nota}{}{nota}</measure></part></score-partwise>"#,
            dir("<words>una corda</words>"),
            dir(r#"<pedal type="sostenuto"/>"#),
            dir(r#"<pedal type="stop"/>"#),
            dir("<words>tre corde</words>"),
        );
        let p = de_texto(&xml).unwrap();
        assert_eq!(p.sostenuto, [(0.0, 2.0)]);
        assert_eq!(p.una_corda, [(0.0, 3.0)]);
        assert!(p.pedais.is_empty());
    }

    #[test]
    fn articulacoes_dedos_e_crescendo() {
        let xml = r#"<score-partwise version="4.0"><part id="P1"><measure number="1">
            <attributes><divisions>1</divisions></attributes>
            <direction><direction-type><dynamics><p/></dynamics></direction-type></direction>
            <direction><direction-type><wedge type="crescendo"/></direction-type></direction>
            <note><pitch><step>C</step><octave>4</octave></pitch><duration>1</duration>
              <notations><articulations><staccato/></articulations><technical><fingering>1</fingering></technical></notations></note>
            <note><pitch><step>D</step><octave>4</octave></pitch><duration>1</duration>
              <notations><articulations><accent/></articulations></notations></note>
            <note><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration>
              <notations><fermata/><technical><fingering>3</fingering></technical></notations></note>
            <direction><direction-type><wedge type="stop"/></direction-type></direction>
            <direction><direction-type><dynamics><f/></dynamics></direction-type></direction>
            <note><pitch><step>F</step><octave>4</octave></pitch><duration>1</duration></note>
        </measure></part></score-partwise>"#;
        let p = de_texto(xml).unwrap();
        let n = &p.notas;
        assert!(n[0].articulacao.staccato && n[1].articulacao.acento && n[2].articulacao.fermata);
        assert_eq!((n[0].dedo, n[1].dedo, n[2].dedo), (Some(1), None, Some(3)));
        // crescendo de p (0,35) até f (0,70): sobe nota a nota
        assert_eq!(n[0].dinamica, 0.35);
        assert!(n[0].dinamica < n[1].dinamica && n[1].dinamica < n[2].dinamica && n[2].dinamica < n[3].dinamica);
        assert_eq!(n[3].dinamica, 0.70);
    }

    #[test]
    fn le_mxl_comprimido() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("cancao-mxl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let caminho = dir.join("ode.mxl");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&caminho).unwrap());
        let op = zip::write::SimpleFileOptions::default();
        z.start_file("META-INF/container.xml", op).unwrap();
        z.write_all(br#"<container><rootfiles><rootfile full-path="partitura/ode.xml"/></rootfiles></container>"#).unwrap();
        z.start_file("partitura/ode.xml", op).unwrap();
        z.write_all(ODE.as_bytes()).unwrap();
        z.finish().unwrap();
        let p = ler(&caminho).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(p.notas.len(), 60);
    }

    #[test]
    fn erros_claros() {
        assert!(de_texto("<score-timewise/>").unwrap_err().contains("score-partwise"));
        assert!(de_texto("<score-partwise>").unwrap_err().contains("XML inválido"));
    }

    #[test]
    fn titulo_pelo_credito_impresso() {
        let xml = r#"<score-partwise><credit page="1"><credit-words>Sonata in B Minor</credit-words></credit>
            <credit page="1"><credit-words>Franz Liszt</credit-words></credit></score-partwise>"#;
        assert_eq!(de_texto(xml).unwrap().titulo, "Sonata in B Minor");
        let xml = r#"<score-partwise><credit><credit-words>Autor</credit-words></credit>
            <credit><credit-type>title</credit-type><credit-words>Prelúdio</credit-words></credit></score-partwise>"#;
        assert_eq!(de_texto(xml).unwrap().titulo, "Prelúdio");
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
