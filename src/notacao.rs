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

//! Notação tradicional: a partitura gravada pelo Verovio, ligada às notas
//! da nossa [`Partitura`].
//!
//! O Verovio (LGPL-3.0, compilado junto pelo `verovioxide`) lê o MusicXML e
//! diagrama a partitura em sistemas, um SVG por sistema. Do mapa de tempos
//! dele vem a posição de cada nota em semínimas; do MEI (o formato interno
//! dele, com os mesmos identificadores) vem a altura; do SVG, com caixas
//! delimitadoras ligadas, vem onde cada cabeça de nota está desenhada. Com
//! (compasso, posição dentro do compasso, tecla) cada nota gráfica é casada
//! com a nota da `Partitura` que o player toca: é a ponte para destacar o que
//! soa e para navegar clicando em qualquer uma das duas vistas. Casar por
//! compasso, e não pela posição absoluta, é o que torna isso robusto: em
//! arquivos reais o Verovio e o nosso leitor às vezes discordam de alguma
//! duração, e a diferença fica presa naquele compasso em vez de se acumular
//! pelo resto da obra.
//!
//! A rasterização usa o `resvg` com as fontes do sistema e a fonte musical
//! Leipzig (que o Verovio traz só em WOFF2, decodificada aqui), necessária
//! para dinâmicas e outros textos musicais.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use base64::Engine;
use resvg::{tiny_skia, usvg};
use roxmltree::Document;
use verovioxide::{BreakMode, FooterMode, HeaderMode, Options, Toolkit};

use crate::partitura::Partitura;

/// Um sistema musical (uma linha da partitura com as duas pautas).
pub struct Sistema {
    pub svg: String,
    /// tamanho do desenho em px lógicos
    pub largura: f64,
    pub altura: f64,
    /// semínimas da primeira e da última nota que começam nele
    pub inicio: f64,
    pub fim: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotaGrafica {
    pub sistema: usize,
    /// caixa da cabeça da nota, em px lógicos dentro do sistema
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// posição em semínimas e tecla (0..87)
    pub tempo: f64,
    pub tecla: u8,
    /// compasso (a partir de 0, na ordem do arquivo) e posição dentro dele
    pub compasso: Option<usize>,
    pub no_compasso: f64,
    /// nota correspondente em `Partitura::notas` (None: ornamento, nota
    /// fora das 88 teclas ou que o leitor simplificado ignora)
    pub indice: Option<usize>,
}

pub struct Notacao {
    pub sistemas: Vec<Sistema>,
    pub notas: Vec<NotaGrafica>,
    /// notas gráficas casadas com a partitura, para diagnóstico
    pub casadas: usize,
}

impl Notacao {
    /// Sistema onde está (ou começa) a posição `tempo`.
    pub fn sistema_em(&self, tempo: f64) -> usize {
        self.sistemas.iter().rposition(|s| s.inicio <= tempo + 1e-6).unwrap_or(0)
    }
}

/// Diagrama a partitura do MusicXML para uma largura em px lógicos.
/// `escala`: tamanho da notação em % (40 = padrão do painel).
pub fn gravar(musicxml: &str, largura_px: u32, escala: u32, p: &Partitura) -> Result<Notacao, String> {
    // os avisos do Verovio (comuns em arquivos exportados) iriam para o terminal
    Toolkit::enable_log(false);
    let mut tk = Toolkit::new().map_err(|e| e.to_string())?;
    let opcoes = Options::builder()
        .scale(escala)
        .page_width(largura_px * 100 / escala)
        // páginas baixas: um sistema por página, ajustado à altura do conteúdo
        .page_height(600)
        .adjust_page_height(true)
        .breaks(BreakMode::Auto)
        .page_margin(50)
        // espaço à esquerda para o nome do instrumento
        .page_margin_left(300)
        .header(HeaderMode::None)
        .footer(FooterMode::None)
        .svg_bounding_boxes(true)
        .build();
    tk.set_options(&opcoes).map_err(|e| e.to_string())?;
    tk.load_data(musicxml).map_err(|e| format!("Verovio não leu a partitura: {e}"))?;

    let alturas = alturas_midi(&tk.get_mei().map_err(|e| e.to_string())?)?;
    let mapa = tk.render_to_timemap_with_options(r#"{"includeMeasures": true}"#).map_err(|e| e.to_string())?;
    let (tempos, inicio_compasso) = tempos_por_nota(&mapa)?;
    // ordem dos compassos: a do desenho, página após página
    let mut ordem_compasso: HashMap<String, usize> = HashMap::new();

    let mut sistemas = Vec::new();
    let mut notas = Vec::new();
    for pagina in 1..=tk.page_count() {
        let svg = tk.render_to_svg(pagina).map_err(|e| e.to_string())?;
        let (largura, altura, compassos, caixas) = caixas_das_notas(&svg)?;
        for c in compassos {
            let n = ordem_compasso.len();
            ordem_compasso.entry(c).or_insert(n);
        }
        let i = sistemas.len();
        for (id, compasso_id, (x, y, w, h)) in caixas {
            let (Some(&tempo), Some(&midi)) = (tempos.get(&id), alturas.get(&id)) else { continue };
            if !(21..=108).contains(&midi) {
                continue;
            }
            let compasso = compasso_id.as_ref().and_then(|c| ordem_compasso.get(c)).copied();
            let no_compasso = compasso_id.and_then(|c| inicio_compasso.get(&c).map(|q| tempo - q)).unwrap_or(0.0);
            notas.push(NotaGrafica { sistema: i, x, y, w, h, tempo, tecla: (midi - 21) as u8, compasso, no_compasso, indice: None });
        }
        sistemas.push(Sistema { svg, largura, altura, inicio: 0.0, fim: 0.0 });
    }
    let casadas = casar(&mut notas, p);

    // limites de cada sistema na linha do tempo do player, só pelas notas
    // casadas (as outras podem ter posições sem sentido, até negativas, que
    // o Verovio calcula para durações que não entende); nunca antes do anterior
    let mut anterior = 0.0;
    for (i, s) in sistemas.iter_mut().enumerate() {
        let tempos = notas.iter().filter(|g| g.sistema == i && g.indice.is_some()).map(|g| g.tempo);
        let (ini, fim) = tempos.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), t| (a.min(t), b.max(t)));
        // sistema sem notas casadas: continua de onde o anterior parou
        (s.inicio, s.fim) = if ini.is_finite() { (ini.max(anterior), fim.max(anterior)) } else { (anterior, anterior) };
        anterior = s.inicio;
    }
    Ok(Notacao { sistemas, notas, casadas })
}

/// Liga cada nota gráfica à nota da partitura com a mesma tecla que começa
/// no mesmo ponto do mesmo compasso; a continuação de uma ligadura (que o
/// nosso leitor junta numa nota só) vai para a nota que ainda soa ali.
fn casar(notas: &mut [NotaGrafica], p: &Partitura) -> usize {
    let chave = |t: f64| (t * 1e3).round() as i64;
    // instante (na nossa linha do tempo) de uma nota gráfica
    let instante = |g: &NotaGrafica| match g.compasso.and_then(|c| p.inicios_compasso.get(c)) {
        Some(inicio) => inicio + g.no_compasso,
        None => g.tempo,
    };
    let mut por_inicio: HashMap<(u8, i64), usize> = HashMap::new();
    let mut por_tecla: HashMap<u8, Vec<usize>> = HashMap::new();
    for (i, n) in p.notas.iter().enumerate() {
        por_inicio.insert((n.tecla, chave(n.inicio)), i);
        por_tecla.entry(n.tecla).or_default().push(i);
    }
    let mut casadas = 0;
    for g in notas.iter_mut() {
        let t = instante(g);
        g.indice = por_inicio.get(&(g.tecla, chave(t))).copied().or_else(|| {
            por_tecla.get(&g.tecla)?.iter().copied().find(|&i| {
                let n = &p.notas[i];
                n.inicio < t && t < n.inicio + n.duracao
            })
        });
        // na nossa linha do tempo, para destacar junto com o piano roll
        if g.compasso.is_some() {
            g.tempo = t;
        }
        casadas += g.indice.is_some() as usize;
    }
    casadas
}

/// Altura MIDI de cada nota do MEI (nome, oitava e alteração, escrita ou
/// implícita pela armadura).
fn alturas_midi(mei: &str) -> Result<HashMap<String, i32>, String> {
    let doc = Document::parse_with_options(mei, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() })
        .map_err(|e| format!("MEI: {e}"))?;
    let alteracao = |a: &str| match a {
        "s" | "su" | "sd" => 1,
        "f" | "fu" | "fd" => -1,
        "ss" | "x" => 2,
        "ff" => -2,
        _ => 0,
    };
    let mut alturas = HashMap::new();
    for n in doc.descendants().filter(|n| n.has_tag_name("note")) {
        let Some(id) = n.attribute(("http://www.w3.org/XML/1998/namespace", "id")) else { continue };
        let nome = n.attribute("pname.ges").or(n.attribute("pname"));
        let oitava = n.attribute("oct.ges").or(n.attribute("oct")).and_then(|o| o.parse::<i32>().ok());
        let (Some(nome), Some(oitava)) = (nome, oitava) else { continue };
        let passo = match nome {
            "c" => 0,
            "d" => 2,
            "e" => 4,
            "f" => 5,
            "g" => 7,
            "a" => 9,
            "b" => 11,
            _ => continue,
        };
        // alteração que soa: a do elemento <accid> (escrita) ou a implícita
        let filho = n.children().find(|c| c.has_tag_name("accid"));
        let acc = filho
            .and_then(|a| a.attribute("accid.ges").or(a.attribute("accid")))
            .or(n.attribute("accid.ges"))
            .or(n.attribute("accid"))
            .map_or(0, alteracao);
        alturas.insert(id.to_string(), (oitava + 1) * 12 + passo + acc);
    }
    Ok(alturas)
}

/// Posição (semínimas) em que cada nota e cada compasso começam, pelo mapa
/// de tempos.
#[allow(clippy::type_complexity)]
fn tempos_por_nota(mapa: &str) -> Result<(HashMap<String, f64>, HashMap<String, f64>), String> {
    let v: serde_json::Value = serde_json::from_str(mapa).map_err(|e| format!("mapa de tempos: {e}"))?;
    let mut tempos = HashMap::new();
    let mut compassos = HashMap::new();
    for evento in v.as_array().into_iter().flatten() {
        let Some(q) = evento.get("qstamp").and_then(|q| q.as_f64()) else { continue };
        for id in evento.get("on").and_then(|o| o.as_array()).into_iter().flatten() {
            if let Some(id) = id.as_str() {
                tempos.insert(id.to_string(), q);
            }
        }
        if let Some(c) = evento.get("measureOn").and_then(|c| c.as_str()) {
            compassos.insert(c.to_string(), q);
        }
    }
    Ok((tempos, compassos))
}

/// Tamanho do SVG (px), compassos (ids, na ordem do desenho) e, para cada
/// nota, id, compasso e caixa (x, y, w, h em px).
///
/// O Verovio desenha num `<svg class="definition-scale">` com unidades
/// próprias (viewBox), deslocado pela margem da página (`g.page-margin`); com
/// `svg_bounding_boxes` cada nota tem um `<g id="bbox-ID"><rect/></g>`.
#[allow(clippy::type_complexity)]
fn caixas_das_notas(svg: &str) -> Result<(f64, f64, Vec<String>, Vec<(String, Option<String>, (f64, f64, f64, f64))>), String> {
    let doc = Document::parse(svg).map_err(|e| format!("SVG: {e}"))?;
    let raiz = doc.root_element();
    let px = |a: Option<&str>| a.and_then(|v| v.trim_end_matches("px").parse::<f64>().ok());
    let (largura, altura) = (px(raiz.attribute("width")).unwrap_or(0.0), px(raiz.attribute("height")).unwrap_or(0.0));
    let interno = doc
        .descendants()
        .find(|n| n.attribute("class") == Some("definition-scale"))
        .ok_or("SVG sem definition-scale")?;
    let vb: Vec<f64> = interno.attribute("viewBox").unwrap_or("").split_whitespace().filter_map(|v| v.parse().ok()).collect();
    let escala = if vb.len() == 4 && vb[2] > 0.0 { largura / vb[2] } else { 1.0 };
    let (mx, my) = doc
        .descendants()
        .find(|n| n.attribute("class") == Some("page-margin"))
        .and_then(|g| g.attribute("transform"))
        .and_then(|t| {
            let dentro = t.strip_prefix("translate(")?.strip_suffix(')')?;
            let mut v = dentro.split(',').filter_map(|x| x.trim().parse::<f64>().ok());
            Some((v.next()?, v.next().unwrap_or(0.0)))
        })
        .unwrap_or((0.0, 0.0));

    let compassos: Vec<String> = doc
        .descendants()
        .filter(|n| n.attribute("class") == Some("measure"))
        .filter_map(|n| n.attribute("id").map(String::from))
        .collect();
    let mut caixas = Vec::new();
    for g in doc.descendants().filter(|n| n.attribute("class") == Some("note bounding-box")) {
        let Some(id) = g.attribute("id").and_then(|i| i.strip_prefix("bbox-")) else { continue };
        let Some(r) = g.children().find(|c| c.has_tag_name("rect")) else { continue };
        let num = |a| r.attribute(a).and_then(|v: &str| v.parse::<f64>().ok()).unwrap_or(0.0);
        let compasso = g.ancestors().find(|a| a.attribute("class") == Some("measure")).and_then(|a| a.attribute("id")).map(String::from);
        caixas.push((
            id.to_string(),
            compasso,
            ((num("x") + mx) * escala, (num("y") + my) * escala, num("width") * escala, num("height") * escala),
        ));
    }
    Ok((largura, altura, compassos, caixas))
}

/// Fontes para a rasterização: as do sistema e a Leipzig do Verovio.
fn fontes() -> Arc<usvg::fontdb::Database> {
    static FONTES: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTES
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            if let Some(ttf) = leipzig_ttf() {
                db.load_font_data(ttf);
            }
            Arc::new(db)
        })
        .clone()
}

/// A Leipzig vem como WOFF2 em base64 dentro de um CSS; o resvg lê TTF.
fn leipzig_ttf() -> Option<Vec<u8>> {
    let css = verovioxide_data::resource_dir().get_file("Leipzig.css")?.contents_utf8()?;
    let ini = css.find("base64,")? + 7;
    let fim = ini + css[ini..].find(')')?;
    let woff2 = base64::engine::general_purpose::STANDARD.decode(&css[ini..fim]).ok()?;
    woofwoof::decompress(&woff2)
}

/// Desenha um sistema em RGBA. `fator`: px físicos por px lógico.
/// `tinta` e `papel`: cores (RGB) das linhas e do fundo.
pub fn rasterizar(sistema: &Sistema, fator: f32, tinta: [u8; 3], papel: [u8; 3]) -> Result<(u32, u32, Vec<u8>), String> {
    // o Verovio desenha com currentColor; troca pela cor da tinta
    let cor = format!("#{:02x}{:02x}{:02x}", tinta[0], tinta[1], tinta[2]);
    let svg = sistema.svg.replacen("color=\"black\"", &format!("color=\"{cor}\""), 1);
    let opcoes = usvg::Options { fontdb: fontes(), ..usvg::Options::default() };
    let arvore = usvg::Tree::from_str(&svg, &opcoes).map_err(|e| e.to_string())?;
    let (w, h) = ((sistema.largura * fator as f64).ceil() as u32, (sistema.altura * fator as f64).ceil() as u32);
    let mut pix = tiny_skia::Pixmap::new(w.max(1), h.max(1)).ok_or("sistema sem tamanho")?;
    pix.fill(tiny_skia::Color::from_rgba8(papel[0], papel[1], papel[2], 255));
    resvg::render(&arvore, tiny_skia::Transform::from_scale(fator, fator), &mut pix.as_mut());
    Ok((pix.width(), pix.height(), pix.take()))
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::musicxml;

    const ODE: &str = include_str!("../exemplos/ode.musicxml");

    #[test]
    fn ode_gravada_e_casada_com_a_partitura() {
        let p = musicxml::de_texto(ODE).unwrap();
        let n = gravar(ODE, 1150, 40, &p).unwrap();
        assert!(!n.sistemas.is_empty());
        // 61 notas escritas: a continuação da ligadura também aparece
        assert_eq!(n.notas.len(), 61);
        assert_eq!(n.casadas, 61, "todas casadas com a partitura");
        // a ligadura do fim: as duas cabeças apontam para a mesma nota
        let c4 = crate::corda::tecla_de_nome("C4").unwrap() as u8;
        let ligadas: Vec<_> = n.notas.iter().filter(|g| g.tecla == c4 && g.tempo >= 30.0).collect();
        assert_eq!(ligadas.len(), 2);
        assert_eq!(ligadas[0].indice, ligadas[1].indice);
        // caixas dentro do desenho do sistema
        for g in &n.notas {
            let s = &n.sistemas[g.sistema];
            assert!(g.x >= 0.0 && g.x + g.w <= s.largura && g.y >= 0.0 && g.y + g.h <= s.altura, "{g:?}");
        }
    }

    #[test]
    fn sistemas_em_ordem_e_achados_pela_posicao() {
        let p = musicxml::de_texto(ODE).unwrap();
        // estreito: força vários sistemas
        let n = gravar(ODE, 500, 40, &p).unwrap();
        assert!(n.sistemas.len() > 2, "{} sistemas", n.sistemas.len());
        assert!(n.sistemas.windows(2).all(|w| w[0].inicio <= w[1].inicio));
        for g in n.notas.iter().filter(|g| g.indice.is_some()) {
            let s = n.sistema_em(g.tempo);
            assert!(s == g.sistema || n.sistemas[s].inicio == n.sistemas[g.sistema].inicio, "{g:?} achou {s}");
        }
    }

    #[test]
    fn mais_aguda_fica_mais_alta_no_desenho() {
        let p = musicxml::de_texto(ODE).unwrap();
        let n = gravar(ODE, 1150, 40, &p).unwrap();
        // no 1º tempo: E4 (pauta de cima) acima do acorde C3-E3-G3
        let no_inicio: Vec<&NotaGrafica> = n.notas.iter().filter(|g| g.tempo == 0.0).collect();
        let y = |nome| no_inicio.iter().find(|g| g.tecla == crate::corda::tecla_de_nome(nome).unwrap() as u8).unwrap().y;
        assert!(y("E4") < y("G3") && y("G3") < y("E3") && y("E3") < y("C3"));
    }

    #[test]
    fn rasteriza_com_a_fonte_musical() {
        assert!(leipzig_ttf().is_some_and(|t| t.len() > 10_000), "Leipzig decodificada");
        let p = musicxml::de_texto(ODE).unwrap();
        let n = gravar(ODE, 1150, 40, &p).unwrap();
        let (w, h, rgba) = rasterizar(&n.sistemas[0], 1.0, [0, 0, 0], [255, 255, 255]).unwrap();
        assert_eq!((w as f64, h as f64), (n.sistemas[0].largura.ceil(), n.sistemas[0].altura.ceil()));
        // tem tinta: parte dos pixels escuros
        let escuros = rgba.chunks(4).filter(|p| p[0] < 100).count();
        assert!(escuros > 1000, "{escuros} pixels escuros");
    }
}
