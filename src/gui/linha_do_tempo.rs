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

//! Linha do tempo do piano roll: onde cada coisa aparece na tela.
//!
//! Horizontal: tempo musical (semínimas), x = (t − início visível) * zoom.
//! Vertical: uma linha por tecla, a mais aguda (C8) em cima,
//! y = (87 − tecla − topo) * altura da linha.
//!
//! Só o que está dentro da janela visível é entregue ao Slint (viewport
//! culling): a interface nunca instancia as milhares de notas de uma obra.

use crate::corda::{self, NUM_TECLAS};
use crate::dedilhado::{self, Mao};
use crate::partitura::Partitura;
use crate::eventos::Pedal;
use crate::player::{NotaTocada, Pisada, RelogioExecucao};

pub const ZOOM_PADRAO: f64 = 60.0; // px por semínima = 100%
const ZOOM_MIN: f64 = 6.0;
const ZOOM_MAX: f64 = 600.0;
const TECLA_PADRAO: f64 = 12.0; // px por linha
const TECLA_MIN: f64 = 5.0;
const TECLA_MAX: f64 = 32.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Vista {
    /// tempo (semínimas) na borda esquerda
    pub inicio: f64,
    /// px por semínima
    pub px_tempo: f64,
    /// linha (0 = C8) na borda de cima; fracionária durante a rolagem
    pub topo: f64,
    /// px por tecla
    pub px_tecla: f64,
    pub largura: f64,
    pub altura: f64,
}

impl Default for Vista {
    fn default() -> Self {
        Vista { inicio: -0.25, px_tempo: ZOOM_PADRAO, topo: 30.0, px_tecla: TECLA_PADRAO, largura: 1000.0, altura: 600.0 }
    }
}

impl Vista {
    pub fn x(&self, tempo: f64) -> f64 {
        (tempo - self.inicio) * self.px_tempo
    }

    pub fn tempo_de_x(&self, x: f64) -> f64 {
        self.inicio + x / self.px_tempo
    }

    pub fn y(&self, tecla: u8) -> f64 {
        ((NUM_TECLAS - 1 - tecla as usize) as f64 - self.topo) * self.px_tecla
    }

    /// Intervalo de tempo visível.
    pub fn tempos(&self) -> (f64, f64) {
        (self.inicio, self.tempo_de_x(self.largura))
    }

    fn limitar(&mut self) {
        let linhas_visiveis = self.altura / self.px_tecla;
        self.topo = self.topo.clamp(0.0, (NUM_TECLAS as f64 - linhas_visiveis).max(0.0));
        self.inicio = self.inicio.max(-1.0);
    }

    /// Zoom horizontal mantendo parado o tempo sob o cursor.
    pub fn zoom_em(&mut self, fator: f64, x: f64) {
        let t = self.tempo_de_x(x);
        self.px_tempo = (self.px_tempo * fator).clamp(ZOOM_MIN, ZOOM_MAX);
        self.inicio = t - x / self.px_tempo;
        self.limitar();
    }

    /// Zoom vertical mantendo parada a tecla sob o cursor.
    pub fn zoom_vertical_em(&mut self, fator: f64, y: f64) {
        let linha = self.topo + y / self.px_tecla;
        self.px_tecla = (self.px_tecla * fator).clamp(TECLA_MIN, TECLA_MAX);
        self.topo = linha - y / self.px_tecla;
        self.limitar();
    }

    pub fn rolar(&mut self, dx: f64, dy: f64) {
        self.inicio += dx / self.px_tempo;
        self.topo += dy / self.px_tecla;
        self.limitar();
    }

    pub fn redimensionar(&mut self, largura: f64, altura: f64) {
        (self.largura, self.altura) = (largura.max(1.0), altura.max(1.0));
        self.limitar();
    }

    /// Centraliza verticalmente a faixa de teclas usada.
    pub fn centralizar(&mut self, tecla_min: u8, tecla_max: u8) {
        let meio = (NUM_TECLAS - 1) as f64 - (tecla_min as f64 + tecla_max as f64) / 2.0;
        self.topo = meio - self.altura / self.px_tecla / 2.0;
        self.limitar();
    }

    /// Acompanha o playhead: se ele passar de 85% da largura, a vista anda.
    pub fn seguir(&mut self, tempo: f64) {
        let x = self.x(tempo);
        if x > 0.85 * self.largura || x < 0.0 {
            self.inicio = tempo - 0.15 * self.largura / self.px_tempo;
        }
    }

    pub fn zoom_percentual(&self) -> f64 {
        100.0 * self.px_tempo / ZOOM_PADRAO
    }
}

/// Peso de uma linha da grade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Peso {
    Subdivisao,
    Tempo,
    Compasso,
}

pub struct Grade {
    pub linhas: Vec<(f64, Peso)>,
    /// número do compasso e sua posição x
    pub rotulos: Vec<(f64, String)>,
    /// menor divisão desenhada: "1/16", "1/4" ou "compasso"
    pub resolucao: &'static str,
}

pub fn grade(v: &Vista, p: &Partitura) -> Grade {
    let (t0, t1) = v.tempos();
    let inicios = p.inicios_de_compasso(t1 + 8.0);
    let mostra_tempos = v.px_tempo >= 14.0;
    let mostra_sub = v.px_tempo / 4.0 >= 10.0;
    let mut linhas = Vec::new();
    let mut rotulos = Vec::new();
    // compassos estreitos: rótulo só a cada tantos compassos
    let largura_compasso = v.px_tempo * 4.0;
    let passo_rotulo = (40.0 / largura_compasso).ceil().max(1.0) as usize;
    for (i, &ini) in inicios.iter().enumerate() {
        let fim = inicios.get(i + 1).copied().unwrap_or(ini + 4.0);
        if fim < t0 || ini > t1 {
            continue;
        }
        linhas.push((v.x(ini), Peso::Compasso));
        if i % passo_rotulo == 0 {
            rotulos.push((v.x(ini), (i + 1).to_string()));
        }
        let (_, unidade) = p.formula_em(ini);
        let tempo = 4.0 / unidade as f64; // duração de um tempo em semínimas
        let passo = if mostra_sub { 0.25 } else { tempo };
        if mostra_tempos {
            let mut t = ini + passo;
            while t < fim - 1e-9 {
                let no_tempo = ((t - ini) / tempo).fract().abs() < 1e-6;
                linhas.push((v.x(t), if no_tempo { Peso::Tempo } else { Peso::Subdivisao }));
                t += passo;
            }
        }
    }
    let resolucao = if mostra_sub { "1/16" } else if mostra_tempos { "1/4" } else { "compasso" };
    Grade { linhas, rotulos, resolucao }
}

/// Como uma nota está em relação ao playhead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Estado {
    Futura,
    Soando,
    Tocada,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotaDesenho {
    /// índice em `Partitura::notas`
    pub indice: usize,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub voz: u8,
    pub mao: Option<Mao>,
    /// dedo escolhido pelo player (vazio se a nota não foi tocada)
    pub dedo: Option<u8>,
    pub vel: f32,
    pub estado: Estado,
}

/// Uma nota tocada para cada nota escrita (None se o player a descartou).
/// A mesma tecla escrita em duas vozes ao mesmo tempo é tocada uma vez só,
/// e as duas notas escritas mostram esse mesmo toque.
pub fn tocadas_por_nota<'a>(p: &Partitura, tocadas: &'a [NotaTocada]) -> Vec<Option<&'a NotaTocada>> {
    let mut v = vec![None; p.notas.len()];
    for t in tocadas {
        v[t.indice] = Some(t);
    }
    let mut por_toque: std::collections::HashMap<(u8, i64), &NotaTocada> = std::collections::HashMap::new();
    for t in tocadas {
        por_toque.insert((t.tecla, (t.tempo * 1e6).round() as i64), t);
    }
    for (i, n) in p.notas.iter().enumerate() {
        if v[i].is_none() {
            v[i] = por_toque.get(&(n.tecla, (n.inicio * 1e6).round() as i64)).copied();
        }
    }
    v
}

pub fn estado_em(inicio: f64, duracao: f64, agora: Option<f64>) -> Estado {
    match agora {
        Some(t) if t >= inicio + duracao => Estado::Tocada,
        Some(t) if t >= inicio => Estado::Soando,
        _ => Estado::Futura,
    }
}

/// Notas que aparecem na vista (viewport culling).
pub fn notas_visiveis(v: &Vista, p: &Partitura, tocadas: &[Option<&NotaTocada>], agora: Option<f64>) -> Vec<NotaDesenho> {
    let (t0, t1) = v.tempos();
    p.notas
        .iter()
        .enumerate()
        .filter(|(_, n)| n.inicio <= t1 && n.inicio + n.duracao >= t0)
        .filter_map(|(i, n)| {
            let y = v.y(n.tecla);
            if y + v.px_tecla < 0.0 || y > v.altura {
                return None;
            }
            let t = tocadas[i];
            Some(NotaDesenho {
                indice: i,
                x: v.x(n.inicio),
                y: y + 1.0,
                w: (n.duracao * v.px_tempo - 1.0).max(2.0),
                h: (v.px_tecla - 2.0).max(1.0),
                voz: n.voz,
                mao: t.map(|t| t.mao),
                dedo: t.map(|t| t.dedo),
                vel: t.map_or(n.dinamica, |t| t.vel),
                estado: estado_em(n.inicio, n.duracao, agora),
            })
        })
        .collect()
}

pub struct TeclaDesenho {
    pub y: f64,
    pub h: f64,
    pub preta: bool,
    /// nome nos Dós ("C4"), vazio nas outras
    pub rotulo: String,
    pub tecla: u8,
    /// a tecla de baixo também é branca (C sobre B, F sobre E)
    pub divisao: bool,
}

pub fn teclas_visiveis(v: &Vista) -> Vec<TeclaDesenho> {
    (0..NUM_TECLAS as u8)
        .filter_map(|t| {
            let y = v.y(t);
            if y + v.px_tecla < 0.0 || y > v.altura {
                return None;
            }
            let nome = corda::nome(t as usize);
            let natural = !nome.contains('#');
            let divisao = natural && (nome.starts_with('C') || nome.starts_with('F'));
            let rotulo = if natural && nome.starts_with('C') && v.px_tecla >= 8.0 { nome } else { String::new() };
            Some(TeclaDesenho { y, h: v.px_tecla, preta: dedilhado::preta(t), rotulo, tecla: t, divisao })
        })
        .collect()
}

/// Uma pisada visível na faixa do seu pedal.
pub struct PisadaDesenho {
    /// índice em `Execucao::pedais`
    pub indice: usize,
    /// 0 sustain, 1 sostenuto, 2 una corda
    pub linha: usize,
    pub x: f64,
    pub w: f64,
    pub valor: f32,
}

/// Pisadas visíveis, nos instantes em que o pé de fato abaixa e solta
/// (com o atraso do pedal sincopado e o ritardando).
pub fn pedais_visiveis(v: &Vista, pedais: &[Pisada], relogio: &RelogioExecucao) -> Vec<PisadaDesenho> {
    let (t0, t1) = v.tempos();
    pedais
        .iter()
        .enumerate()
        .map(|(i, d)| (i, d, relogio.tempo_em(d.abaixa_s), relogio.tempo_em(d.solta_s)))
        .filter(|&(_, _, a, s)| a <= t1 && s >= t0)
        .map(|(indice, d, a, s)| PisadaDesenho { indice, linha: d.pedal as usize, x: v.x(a), w: ((s - a) * v.px_tempo - 2.0).max(1.0), valor: d.valor })
        .collect()
}

/// Posição planejada de cada pedal (sustain, sostenuto, una corda) no
/// instante `s` da execução.
pub fn pedais_em(pedais: &[Pisada], s: f64) -> [f32; 3] {
    let mut v = [0f32; 3];
    for d in pedais.iter().filter(|d| d.abaixa_s <= s && s < d.solta_s) {
        v[d.pedal as usize] = v[d.pedal as usize].max(d.valor);
    }
    v
}

pub fn nome_pedal(p: Pedal) -> &'static str {
    match p {
        Pedal::Sustain => "Sustain",
        Pedal::Sostenuto => "Sostenuto",
        Pedal::UnaCorda => "Una corda",
    }
}

/// Texto do inspetor para uma pisada: qual pedal, como e quando.
pub fn descrever_pisada(p: &Partitura, d: &Pisada, relogio: &RelogioExecucao) -> String {
    let (pe, efeito) = match d.pedal {
        Pedal::Sustain => ("direito", "levanta todos os abafadores"),
        Pedal::Sostenuto => ("do meio", "segura os abafadores já levantados quando desce"),
        Pedal::UnaCorda => ("esquerdo", "o martelo acerta uma corda a menos"),
    };
    let quando = |s: f64| {
        let (compasso, tempo) = p.compasso_e_tempo(relogio.tempo_em(s));
        format!("compasso {compasso}, tempo {} ({s:.2} s)", formatar((tempo * 100.0).round() / 100.0))
    };
    let profundidade = match d.valor {
        v if v >= 0.75 => "no fundo",
        v if v > 0.25 => "meio pedal",
        _ => "raso",
    };
    let s = format!(
        "{} (pedal {pe})\nAbaixa: {}\nSolta: {}\nDuração: {:.2} s\nProfundidade: {:.0}% ({profundidade})\n{}\nEfeito: {efeito}",
        nome_pedal(d.pedal),
        quando(d.abaixa_s),
        quando(d.solta_s),
        d.solta_s - d.abaixa_s,
        d.valor * 100.0,
        if d.automatica { "Decidido pelo pé" } else { "Marcado na partitura" },
    );
    s.replace('.', ",")
}

/// Cor de uma voz (contraponto). Paleta distinguível em fundo escuro.
pub fn cor_voz(voz: u8) -> (u8, u8, u8) {
    const PALETA: [(u8, u8, u8); 8] = [
        (90, 170, 250),  // azul
        (240, 140, 70),  // laranja
        (120, 210, 120), // verde
        (220, 110, 200), // magenta
        (90, 200, 200),  // ciano
        (230, 200, 80),  // amarelo
        (170, 140, 240), // lilás
        (240, 110, 110), // vermelho
    ];
    PALETA[(voz.max(1) as usize - 1) % PALETA.len()]
}

pub fn cor_mao(mao: Option<Mao>) -> (u8, u8, u8) {
    match mao {
        Some(Mao::Direita) => (90, 170, 250),
        Some(Mao::Esquerda) => (240, 140, 70),
        None => (140, 140, 140),
    }
}

/// Texto do inspetor para uma nota.
pub fn descrever(p: &Partitura, i: usize, t: Option<&NotaTocada>, cordas: &[corda::Corda]) -> String {
    let n = &p.notas[i];
    let c = &cordas[n.tecla as usize];
    let (compasso, tempo) = p.compasso_e_tempo(n.inicio);
    let mut s = format!(
        "{}  (tecla {})\nFrequência: {:.2} Hz\nCompasso {}, tempo {}\nDuração: {} semínima(s)\nVoz: {}",
        c.nome,
        n.tecla + 1,
        c.parcial(1),
        compasso,
        formatar(tempo),
        formatar(n.duracao),
        n.voz
    );
    match t {
        Some(t) => {
            let mao = if t.mao == Mao::Direita { "direita" } else { "esquerda" };
            s += &format!("\nMão {mao}, dedo {}\nVelocity: {} de 127", t.dedo, (t.vel * 127.0).round() as u8);
        }
        None => s += "\nNão tocada (mão sem dedos livres)",
    }
    s.replace('.', ",")
}

/// Número sem zeros inúteis: 2 -> "2", 2.5 -> "2.5".
fn formatar(x: f64) -> String {
    let s = format!("{x:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::partitura::exemplo;

    fn vista() -> Vista {
        Vista { inicio: 0.0, px_tempo: 50.0, topo: 0.0, px_tecla: 10.0, largura: 500.0, altura: 300.0 }
    }

    #[test]
    fn tres_pedais_na_faixa_no_estado_e_no_inspetor() {
        let mut p = exemplo("acordes").unwrap();
        p.sostenuto = vec![(2.0, 4.0)];
        p.una_corda = vec![(0.0, 2.0)];
        let e = crate::player::Player { humanizacao: 0.0, ..Default::default() }.tocar(&p, 48000);
        let faixas = pedais_visiveis(&vista(), &e.pedais, &e.relogio);
        for linha in 0..3 {
            assert!(faixas.iter().any(|d| d.linha == linha), "faixa {linha}");
        }
        // no meio do 1º acorde: sustain e una corda; no 2º: sustain e sostenuto
        let meio = |t: f64| e.relogio.segundos(t);
        assert_eq!(pedais_em(&e.pedais, meio(1.0)), [1.0, 0.0, 1.0]);
        assert_eq!(pedais_em(&e.pedais, meio(3.0)), [1.0, 1.0, 0.0]);
        let so = e.pedais.iter().find(|d| d.pedal == Pedal::Sostenuto).unwrap();
        let texto = descrever_pisada(&p, so, &e.relogio);
        assert!(texto.starts_with("Sostenuto (pedal do meio)"), "{texto}");
        assert!(texto.contains("Abaixa: compasso 1, tempo 3,07"), "{texto}");
        assert!(texto.contains("Profundidade: 100% (no fundo)"), "{texto}");
        assert!(texto.contains("Marcado na partitura"), "{texto}");
    }

    #[test]
    fn coordenadas() {
        let v = vista();
        assert_eq!(v.x(2.0), 100.0);
        assert_eq!(v.tempo_de_x(100.0), 2.0);
        assert_eq!(v.y(87), 0.0); // C8 em cima
        assert_eq!(v.y(86), 10.0);
    }

    #[test]
    fn zoom_mantem_o_tempo_sob_o_cursor() {
        let mut v = vista();
        let antes = v.tempo_de_x(200.0);
        v.zoom_em(2.0, 200.0);
        assert!((v.tempo_de_x(200.0) - antes).abs() < 1e-9);
        assert_eq!(v.px_tempo, 100.0);
        v.zoom_em(1e6, 0.0);
        assert_eq!(v.px_tempo, ZOOM_MAX);
    }

    #[test]
    fn rolagem_vertical_fica_dentro_do_teclado() {
        let mut v = vista();
        v.rolar(0.0, -1000.0);
        assert_eq!(v.topo, 0.0);
        v.rolar(0.0, 1e6);
        assert_eq!(v.topo, 88.0 - 30.0); // 300 px / 10 px = 30 linhas visíveis
    }

    #[test]
    fn so_entram_as_notas_visiveis() {
        let p = exemplo("escala").unwrap();
        let tocadas = tocadas_por_nota(&p, &[]);
        let mut v = vista();
        v.centralizar(27, 51);
        let tudo = Vista { largura: 1e6, altura: 1e6, topo: 0.0, ..v.clone() };
        let todas = notas_visiveis(&tudo, &p, &tocadas, None);
        let algumas = notas_visiveis(&v, &p, &tocadas, None);
        assert_eq!(todas.len(), p.notas.len());
        assert!(algumas.len() < todas.len() && !algumas.is_empty());
        assert!(algumas.iter().all(|n| n.x <= v.largura && n.x + n.w >= 0.0));
    }

    #[test]
    fn estado_em_relacao_ao_playhead() {
        assert_eq!(estado_em(1.0, 1.0, None), Estado::Futura);
        assert_eq!(estado_em(1.0, 1.0, Some(0.5)), Estado::Futura);
        assert_eq!(estado_em(1.0, 1.0, Some(1.5)), Estado::Soando);
        assert_eq!(estado_em(1.0, 1.0, Some(2.0)), Estado::Tocada);
    }

    #[test]
    fn grade_conforme_o_zoom() {
        let p = exemplo("escala").unwrap();
        let mut v = vista();
        v.px_tempo = 30.0; // semicolcheia de 7,5 px: só tempos
        assert_eq!(grade(&v, &p).resolucao, "1/4");
        v.px_tempo = 60.0; // semicolcheia de 15 px
        let g = grade(&v, &p);
        assert_eq!(g.resolucao, "1/16");
        assert_eq!(g.rotulos[0].1, "1");
        v.px_tempo = 8.0;
        assert_eq!(grade(&v, &p).resolucao, "compasso");
    }
}
