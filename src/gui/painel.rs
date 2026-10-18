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

//! Painel da partitura tradicional (Live Score Panel).
//!
//! A partitura é diagramada pelo Verovio numa thread à parte, ao abrir a
//! peça (e de novo quando o zoom, a largura ou o modo mudam): nunca a cada
//! nota. Os sistemas são rasterizados só quando aparecem e ficam em cache. A
//! posição vem do mesmo relógio do piano roll (o do motor de áudio); o painel
//! só lê. Mexer no painel nunca interfere no áudio.
//!
//! Dois modos:
//!
//! - normal: os sistemas empilhados, como numa página, com um cursor que
//!   anda entre as colunas de notas;
//! - alinhado ao tempo: os sistemas lado a lado numa tira deformada para que
//!   cada coluna de notas fique exatamente acima da sua barra no piano roll
//!   (x = (t - início) * zoom, a mesma régua do roll). As cabeças de nota
//!   ficam em escala 1; só o espaço entre as colunas estica ou encolhe, e
//!   onde não há espaço o trecho é comprimido inteiro.

use std::collections::HashMap;
use std::sync::mpsc;

use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use super::linha_do_tempo::Vista;
use super::{FatiaVis, MarcaVis, PianoRoll, SistemaVis};
use crate::notacao::{self, Ancora, Notacao};
use crate::partitura::Partitura;

const ESCALA_PADRAO: u32 = 40;
const ESCALA_MIN: u32 = 25;
const ESCALA_MAX: u32 = 80;
/// espaço entre sistemas (px)
const ENTRE_SISTEMAS: f64 = 6.0;
const TINTA: [u8; 3] = [24, 24, 26];
const PAPEL: [u8; 3] = [233, 229, 218];

/// pedido de diagramação: (largura, escala, proporcional)
type Pedido = (u32, u32, bool);

pub struct Painel {
    /// MusicXML da peça (sem ele não há partitura gráfica)
    fonte: Option<String>,
    partitura: Partitura,
    notacao: Option<Notacao>,
    /// diagramação em andamento e seu resultado
    pedido: Option<(Pedido, mpsc::Receiver<Result<Notacao, String>>)>,
    gravada: Option<Pedido>,
    escala: u32,
    /// imagens dos sistemas já rasterizados (no fator de pixels atual)
    cache: HashMap<usize, Image>,
    fator: f32,
    /// deslocamento vertical do conteúdo no modo normal (px)
    rolagem: f64,
    /// nota a mostrar (seleção vinda do piano roll)
    mostrar: Option<usize>,
    /// última posição vista: um salto (com a música parada) também é seguido
    ultimo_agora: Option<f64>,
    status: String,
    /// chave do que foi entregue ao Slint (para não refazer à toa)
    desenhado: Option<String>,
}

impl Painel {
    pub fn new(fonte: Option<String>, partitura: Partitura) -> Self {
        let status = if fonte.is_some() { "Diagramando a partitura...".into() } else { "Partitura gráfica só para arquivos MusicXML.".into() };
        Painel {
            fonte,
            partitura,
            notacao: None,
            pedido: None,
            gravada: None,
            escala: ESCALA_PADRAO,
            cache: HashMap::new(),
            fator: 1.0,
            rolagem: 0.0,
            mostrar: None,
            ultimo_agora: None,
            status,
            desenhado: None,
        }
    }

    pub fn zoom(&mut self, passo: i32) {
        self.escala = (self.escala as i32 + passo).clamp(ESCALA_MIN as i32, ESCALA_MAX as i32) as u32;
    }

    pub fn rolar(&mut self, dy: f64) {
        self.rolagem = (self.rolagem - dy).clamp(0.0, self.altura_total());
        self.desenhado = None;
    }

    /// Faz o painel mostrar a nota (seleção vinda do piano roll).
    pub fn mostrar(&mut self, indice: Option<usize>) {
        self.mostrar = indice;
        self.desenhado = None;
    }

    /// Nota da partitura e posição (semínimas) de uma marca clicada.
    pub fn nota_da_marca(&self, id: usize) -> Option<(Option<usize>, f64)> {
        let g = self.notacao.as_ref()?.notas.get(id)?;
        Some((g.indice, g.tempo))
    }

    fn altura_total(&self) -> f64 {
        self.notacao.as_ref().map_or(0.0, |n| n.sistemas.iter().map(|s| s.altura + ENTRE_SISTEMAS).sum())
    }

    /// topo (px) de cada sistema no conteúdo empilhado
    fn topos(&self) -> Vec<f64> {
        let mut y = 0.0;
        self.notacao
            .as_ref()
            .map(|n| {
                n.sistemas
                    .iter()
                    .map(|s| {
                        let t = y;
                        y += s.altura + ENTRE_SISTEMAS;
                        t
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Pede uma diagramação nova se a largura, o zoom ou o modo mudaram.
    fn diagramar_se_preciso(&mut self, largura: u32, proporcional: bool) {
        let Some(fonte) = &self.fonte else { return };
        if largura < 200 {
            return;
        }
        let alvo = (largura, self.escala, proporcional);
        let parecido = |a: Pedido| a.1 == alvo.1 && a.2 == alvo.2 && (a.0 as f64 - alvo.0 as f64).abs() < 0.08 * alvo.0 as f64;
        if self.gravada.is_some_and(parecido) {
            return;
        }
        if self.pedido.is_some() {
            return; // espera a atual terminar e decide de novo
        }
        let (tx, rx) = mpsc::channel();
        let (fonte, partitura) = (fonte.clone(), self.partitura.clone());
        std::thread::spawn(move || {
            let _ = tx.send(notacao::gravar(&fonte, alvo.0, alvo.1, alvo.2, &partitura));
        });
        self.pedido = Some((alvo, rx));
    }

    fn receber(&mut self) {
        let Some((alvo, rx)) = &self.pedido else { return };
        let Ok(resultado) = rx.try_recv() else { return };
        self.gravada = Some(*alvo);
        self.pedido = None;
        self.aceitar(resultado);
    }

    fn aceitar(&mut self, resultado: Result<Notacao, String>) {
        match resultado {
            Ok(n) => {
                self.status = format!("{} sistemas, {} de {} notas ligadas ao piano roll", n.sistemas.len(), n.casadas, n.notas.len());
                self.notacao = Some(n);
                self.cache.clear();
                self.rolagem = self.rolagem.min(self.altura_total());
            }
            Err(e) => self.status = format!("Partitura indisponível: {e}"),
        }
        self.desenhado = None;
    }

    fn imagem(&mut self, i: usize) -> Option<Image> {
        if let Some(img) = self.cache.get(&i) {
            return Some(img.clone());
        }
        let s = self.notacao.as_ref()?.sistemas.get(i)?;
        let (w, h, rgba) = notacao::rasterizar(s, self.fator, TINTA, PAPEL).ok()?;
        let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, w, h);
        let img = Image::from_rgba8(buf);
        // só os sistemas recentes ficam na memória
        if self.cache.len() > 40 {
            self.cache.clear();
        }
        self.cache.insert(i, img.clone());
        Some(img)
    }

    /// Estado de cada nota desenhada: 1 soando, 2 selecionada (as outras ficam de fora).
    fn estados(&self, agora: Option<f64>, selecionada: Option<usize>) -> HashMap<usize, i32> {
        let Some(n) = &self.notacao else { return HashMap::new() };
        n.notas
            .iter()
            .enumerate()
            .filter_map(|(j, g)| {
                let i = g.indice?;
                let nota = &self.partitura.notas[i];
                let soando = agora.is_some_and(|t| g.tempo <= t + 1e-9 && t < nota.inicio + nota.duracao);
                let estado = if selecionada == Some(i) { 2 } else if soando { 1 } else { 0 };
                (estado != 0).then_some((j, estado))
            })
            .collect()
    }

    /// Chamado a cada quadro: `agora` em semínimas (None antes de tocar);
    /// `vista`: a régua do piano roll, usada no modo alinhado.
    pub fn atualizar(&mut self, ui: &PianoRoll, vista: &Vista, agora: Option<f64>, tocando: bool, selecionada: Option<usize>) {
        let fator = ui.window().scale_factor();
        if (fator - self.fator).abs() > 1e-3 {
            self.fator = fator;
            self.cache.clear();
            self.desenhado = None;
        }
        let alinhado = ui.get_alinhado();
        if ui.get_modo_partitura() != 0 {
            self.diagramar_se_preciso(ui.get_partitura_largura() as u32, alinhado);
        }
        self.receber();
        ui.set_status_partitura(self.status.clone().into());
        ui.set_zoom_partitura(format!("{}%", self.escala * 100 / ESCALA_PADRAO).into());
        if self.notacao.is_none() {
            ui.set_sistemas(ModelRc::new(VecModel::from(Vec::<SistemaVis>::new())));
            ui.set_fatias(ModelRc::new(VecModel::from(Vec::<FatiaVis>::new())));
            ui.set_marcas(ModelRc::new(VecModel::from(Vec::<MarcaVis>::new())));
            ui.set_cursor_visivel(false);
            return;
        }
        if alinhado {
            self.atualizar_alinhado(ui, vista, agora, selecionada);
        } else {
            self.atualizar_normal(ui, agora, tocando, selecionada);
        }
    }

    fn atualizar_normal(&mut self, ui: &PianoRoll, agora: Option<f64>, tocando: bool, selecionada: Option<usize>) {
        let altura_vista = ui.get_partitura_altura() as f64;
        let largura_vista = ui.get_partitura_largura() as f64;
        let topos = self.topos();
        let n = self.notacao.as_ref().expect("diagramada");

        // rolagem: segue a execução sistema a sistema, antes da linha acabar;
        // parado, segue só quando a posição salta
        let atual = agora.map(|t| n.sistema_em(t));
        let saltou = agora != self.ultimo_agora;
        self.ultimo_agora = agora;
        if let (true, true, Some(k)) = (ui.get_seguir(), tocando || saltou, atual) {
            let proximo = (k + 1).min(n.sistemas.len() - 1);
            let fim_do_proximo = topos[proximo] + n.sistemas[proximo].altura;
            let cabe_o_proximo = fim_do_proximo - topos[k] <= altura_vista;
            let visivel = topos[k] >= self.rolagem && topos[k] + n.sistemas[k].altura <= self.rolagem + altura_vista;
            let precisa_do_proximo = cabe_o_proximo && fim_do_proximo > self.rolagem + altura_vista;
            if !visivel || precisa_do_proximo {
                self.rolagem = topos[k];
            }
        }
        // seleção vinda do piano roll: rola até a nota
        if let Some(i) = self.mostrar.take() {
            if let Some(g) = n.notas.iter().find(|g| g.indice == Some(i)) {
                let (t, h) = (topos[g.sistema], n.sistemas[g.sistema].altura);
                if t < self.rolagem || t + h > self.rolagem + altura_vista {
                    self.rolagem = t;
                }
            }
        }
        // compacto: a altura do sistema atual
        let k = atual.unwrap_or_else(|| topos.iter().rposition(|&t| t <= self.rolagem).unwrap_or(0));
        ui.set_altura_compacta((n.sistemas[k].altura + 2.0 * ENTRE_SISTEMAS) as f32);
        if ui.get_modo_partitura() == 1 {
            self.rolagem = topos[k];
        }

        // cursor: entre as colunas de notas do sistema atual
        let centraliza = |largura: f64| ((largura_vista - largura) / 2.0).max(0.0);
        let cursor = agora.zip(atual).and_then(|(t, k)| {
            let s = &n.sistemas[k];
            let x = s.x_em(t)?;
            Some((centraliza(s.largura) + x, topos[k] - self.rolagem, s.altura))
        });
        ui.set_cursor_visivel(cursor.is_some());
        if let Some((x, y, h)) = cursor {
            ui.set_cursor_x(x as f32);
            ui.set_cursor_y(y as f32);
            ui.set_cursor_h(h as f32);
        }

        let estados = self.estados(agora, selecionada);
        let mut chave_estados: Vec<_> = estados.iter().map(|(a, b)| (*a, *b)).collect();
        chave_estados.sort_unstable();
        let chave = format!("normal {} {:?} {} {} {}", (self.rolagem * 10.0) as i64, chave_estados, largura_vista as u32, altura_vista as u32, self.cache.is_empty());
        if self.desenhado.as_ref() == Some(&chave) {
            return;
        }

        // sistemas visíveis, centralizados na largura
        let visiveis: Vec<(usize, f64, f64, f64, f64)> = (0..n.sistemas.len())
            .filter(|&i| topos[i] + n.sistemas[i].altura >= self.rolagem && topos[i] <= self.rolagem + altura_vista)
            .map(|i| {
                let s = &n.sistemas[i];
                (i, centraliza(s.largura), topos[i] - self.rolagem, s.largura, s.altura)
            })
            .collect();
        let mut marcas = Vec::new();
        for &(i, x0, y0, _, _) in &visiveis {
            for (j, g) in n.notas.iter().enumerate().filter(|(_, g)| g.sistema == i) {
                marcas.push(MarcaVis {
                    id: j as i32,
                    x: (x0 + g.x) as f32,
                    y: (y0 + g.y) as f32,
                    w: g.w as f32,
                    h: g.h as f32,
                    estado: estados.get(&j).copied().unwrap_or(0),
                });
            }
        }
        let mut sistemas = Vec::new();
        for (i, x0, y0, w, h) in visiveis {
            if let Some(imagem) = self.imagem(i) {
                sistemas.push(SistemaVis { imagem, x: x0 as f32, y: y0 as f32, w: w as f32, h: h as f32 });
            }
        }
        ui.set_sistemas(ModelRc::new(VecModel::from(sistemas)));
        ui.set_fatias(ModelRc::new(VecModel::from(Vec::<FatiaVis>::new())));
        ui.set_marcas(ModelRc::new(VecModel::from(marcas)));
        self.desenhado = Some(chave);
    }

    fn atualizar_alinhado(&mut self, ui: &PianoRoll, vista: &Vista, agora: Option<f64>, selecionada: Option<usize>) {
        self.ultimo_agora = agora;
        self.mostrar = None; // a vista do roll já manda na posição
        let n = self.notacao.as_ref().expect("diagramada");
        let (t0, t1) = vista.tempos();
        // sistemas que tocam o trecho visível (com um de folga de cada lado)
        let primeiro = n.sistema_em(t0).saturating_sub(1);
        let ultimo = (n.sistema_em(t1) + 1).min(n.sistemas.len() - 1);
        let altura = (primeiro..=ultimo).map(|i| n.sistemas[i].altura).fold(0.0, f64::max);
        ui.set_altura_compacta((altura + 2.0 * ENTRE_SISTEMAS) as f32);

        // cursor: o mesmo x do playhead do roll
        let cursor = agora.map(|t| vista.x(t)).filter(|x| *x >= 0.0 && *x <= vista.largura);
        ui.set_cursor_visivel(cursor.is_some());
        if let Some(x) = cursor {
            ui.set_cursor_x(x as f32);
            ui.set_cursor_y(0.0);
            ui.set_cursor_h(altura as f32);
        }

        let estados = self.estados(agora, selecionada);
        let mut chave_estados: Vec<_> = estados.iter().map(|(a, b)| (*a, *b)).collect();
        chave_estados.sort_unstable();
        let chave = format!("alinhado {:?} {:?} {} {}", (vista.inicio, vista.px_tempo, vista.largura), chave_estados, self.fator, self.cache.is_empty());
        if self.desenhado.as_ref() == Some(&chave) {
            return;
        }

        let (pedacos, marcas) = montar_tira(n, vista, primeiro, ultimo, &estados);
        let fator = self.fator as f64;
        let mut fatias = Vec::new();
        for p in pedacos {
            let Some(imagem) = self.imagem(p.sistema) else { continue };
            let altura = self.notacao.as_ref().expect("diagramada").sistemas[p.sistema].altura;
            fatias.push(FatiaVis {
                imagem,
                clip_x: (p.de * fator).round() as i32,
                clip_w: ((p.ate - p.de) * fator).round().max(1.0) as i32,
                clip_h: (altura * fator).round() as i32,
                x: p.x as f32,
                w: p.w as f32,
                h: altura as f32,
            });
        }
        ui.set_sistemas(ModelRc::new(VecModel::from(Vec::<SistemaVis>::new())));
        ui.set_fatias(ModelRc::new(VecModel::from(fatias)));
        ui.set_marcas(ModelRc::new(VecModel::from(marcas)));
        self.desenhado = Some(chave);
    }

    /// Diagrama já, na thread atual (captura sem janela).
    pub fn diagramar_agora(&mut self, largura: u32, proporcional: bool) {
        if let Some(fonte) = &self.fonte {
            let r = notacao::gravar(fonte, largura, self.escala, proporcional, &self.partitura);
            self.gravada = Some((largura, self.escala, proporcional));
            self.aceitar(r);
        }
    }
}

/// Um pedaço da tira: o trecho [de, ate) do sistema (px no desenho), posto em
/// [x, x + w) na tela.
#[derive(Debug, Clone, PartialEq)]
struct Pedaco {
    sistema: usize,
    de: f64,
    ate: f64,
    x: f64,
    w: f64,
}

/// Monta a tira alinhada ao tempo com os sistemas `primeiro..=ultimo`: cada
/// coluna de notas vai para x = vista.x(t), com as cabeças em escala 1; o que
/// fica entre as colunas estica ou encolhe. Devolve os pedaços e as marcas
/// (notas clicáveis/destacadas) já nas coordenadas da tela.
fn montar_tira(n: &Notacao, vista: &Vista, primeiro: usize, ultimo: usize, estados: &HashMap<usize, i32>) -> (Vec<Pedaco>, Vec<MarcaVis>) {
    let mut pedacos = Vec::new();
    // onde termina, na tela, o que já foi desenhado (nada volta para trás)
    let mut fim_desenhado = f64::NEG_INFINITY;
    let mut empurra = |sistema: usize, de: f64, ate: f64, x: f64, w: f64| {
        let (mut de, mut x, mut w) = (de, x, w);
        // corta o que invadiria o pedaço anterior
        if x < fim_desenhado {
            let corte = fim_desenhado - x;
            if corte >= w {
                return;
            }
            de += (ate - de) * corte / w;
            x += corte;
            w -= corte;
        }
        if w >= 0.5 && ate > de {
            if x + w >= 0.0 && x <= vista.largura {
                pedacos.push(Pedaco { sistema, de, ate, x, w });
            }
            fim_desenhado = x + w;
        }
    };
    for k in primeiro..=ultimo {
        let s = &n.sistemas[k];
        let a: &[Ancora] = &s.ancoras;
        if a.is_empty() {
            continue;
        }
        // tela de uma coluna: a borda das cabeças vai para vista.x(t)
        let tela = |c: &Ancora| vista.x(c.tempo) - (c.xn - c.x0);
        // começo do sistema (clave, armadura): em escala 1, terminando na 1ª
        // coluna; nas emendas entre sistemas não cabe e é cortado sozinho
        empurra(k, 0.0, a[0].x0, tela(&a[0]) - a[0].x0, a[0].x0);
        // primeira coluna do sistema seguinte, onde este tem que terminar
        let seguinte = n.sistemas.get(k + 1).and_then(|p| p.ancoras.first()).map(|p| tela(p));
        for (i, c) in a.iter().enumerate() {
            let x = tela(c);
            // a coluna em escala 1
            empurra(k, c.x0, c.x1, x, c.x1 - c.x0);
            // o espaço até a próxima coluna, esticado ou comprimido; depois da
            // última, o resto do sistema (barra final) até a 1ª coluna do seguinte
            let inicio = x + (c.x1 - c.x0);
            match (a.get(i + 1), seguinte) {
                (Some(p), _) => empurra(k, c.x1, p.x0, inicio, tela(p) - inicio),
                (None, Some(fim)) => empurra(k, c.x1, s.largura, inicio, fim - inicio),
                (None, None) => empurra(k, c.x1, s.largura, inicio, s.largura - c.x1),
            }
        }
    }
    // marcas: cada cabeça de nota na posição da sua coluna
    let mut marcas = Vec::new();
    for (j, g) in n.notas.iter().enumerate().filter(|(_, g)| (primeiro..=ultimo).contains(&g.sistema)) {
        let s = &n.sistemas[g.sistema];
        let Some(c) = s.ancoras.iter().find(|c| (c.tempo - g.tempo).abs() < 1e-6) else { continue };
        let x = vista.x(c.tempo) + (g.x - c.xn);
        if x + g.w < 0.0 || x > vista.largura {
            continue;
        }
        marcas.push(MarcaVis { id: j as i32, x: x as f32, y: g.y as f32, w: g.w as f32, h: g.h as f32, estado: estados.get(&j).copied().unwrap_or(0) });
    }
    (pedacos, marcas)
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::musicxml;

    const ODE: &str = include_str!("../../exemplos/ode.musicxml");

    #[test]
    fn tira_poe_cada_coluna_na_regua_do_roll() {
        let p = musicxml::de_texto(ODE).unwrap();
        let n = notacao::gravar(ODE, 700, 40, true, &p).unwrap();
        let vista = Vista { inicio: -0.5, px_tempo: 80.0, topo: 0.0, px_tecla: 12.0, largura: 3000.0, altura: 400.0 };
        let (pedacos, marcas) = montar_tira(&n, &vista, 0, n.sistemas.len() - 1, &HashMap::new());
        // a cabeça de cada coluna fica visível: algum pedaço do seu sistema cobre
        // o trecho do desenho onde ela está, na posição certa da tela
        for s_i in 0..n.sistemas.len() {
            for c in &n.sistemas[s_i].ancoras {
                let alvo = vista.x(c.tempo);
                let coberta = pedacos.iter().any(|p| p.sistema == s_i && p.de <= c.xn + 1e-6 && c.xn < p.ate && (p.x + (c.xn - p.de) * p.w / (p.ate - p.de) - alvo).abs() < 0.5);
                assert!(coberta, "sistema {s_i}: coluna em {} não aparece", c.tempo);
            }
        }
        assert!(!pedacos.is_empty());
        // cada pedaço começa depois do anterior: a tira só anda para a frente
        for par in pedacos.windows(2) {
            assert!(par[1].x >= par[0].x + par[0].w - 1e-6, "{par:?}");
        }
        // com vários sistemas, nenhuma coluna some na emenda
        assert!(n.sistemas.len() > 1, "a Ode em 700 px precisa de mais de um sistema");
        // a borda das cabeças de cada coluna cai exatamente em vista.x(t)
        for s in &n.sistemas {
            for c in &s.ancoras {
                let x = vista.x(c.tempo);
                assert!(marcas.iter().any(|m| (m.x as f64 - x).abs() < 1e-3), "coluna em {} sem nota em x={x}", c.tempo);
            }
        }
    }
}
