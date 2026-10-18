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
//! peça (e de novo quando o zoom ou a largura mudam): nunca a cada nota. Os
//! sistemas são rasterizados só quando aparecem e ficam em cache. A posição
//! vem do mesmo relógio do piano roll (o do motor de áudio); o painel só lê.
//! Mexer no painel nunca interfere no áudio.

use std::collections::HashMap;
use std::sync::mpsc;

use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use super::{MarcaVis, PianoRoll, SistemaVis};
use crate::notacao::{self, Notacao};
use crate::partitura::Partitura;

const ESCALA_PADRAO: u32 = 40;
const ESCALA_MIN: u32 = 25;
const ESCALA_MAX: u32 = 80;
/// espaço entre sistemas (px)
const ENTRE_SISTEMAS: f64 = 6.0;
const TINTA: [u8; 3] = [24, 24, 26];
const PAPEL: [u8; 3] = [233, 229, 218];

pub struct Painel {
    /// MusicXML da peça (sem ele não há partitura gráfica)
    fonte: Option<String>,
    partitura: Partitura,
    notacao: Option<Notacao>,
    /// diagramação em andamento: (largura, escala) pedidas e o resultado
    pedido: Option<(u32, u32, mpsc::Receiver<Result<Notacao, String>>)>,
    gravada: (u32, u32),
    escala: u32,
    /// imagens dos sistemas já rasterizados, com o fator de pixels usado
    cache: HashMap<usize, Image>,
    fator: f32,
    /// deslocamento vertical do conteúdo (px)
    rolagem: f64,
    /// nota a mostrar (seleção vinda do piano roll)
    mostrar: Option<usize>,
    /// última posição vista: um salto (com a música parada) também é seguido
    ultimo_agora: Option<f64>,
    status: String,
    desenhado: Option<(i64, Vec<(usize, i32)>, (u32, u32), usize)>,
}

impl Painel {
    pub fn new(fonte: Option<String>, partitura: Partitura) -> Self {
        let status = if fonte.is_some() { "Diagramando a partitura...".into() } else { "Partitura gráfica só para arquivos MusicXML.".into() };
        Painel {
            fonte,
            partitura,
            notacao: None,
            pedido: None,
            gravada: (0, 0),
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

    /// Pede uma diagramação nova se a largura ou o zoom mudaram.
    fn diagramar_se_preciso(&mut self, largura: u32) {
        let Some(fonte) = &self.fonte else { return };
        if largura < 200 {
            return;
        }
        let alvo = (largura, self.escala);
        let em_curso = self.pedido.as_ref().map(|p| (p.0, p.1));
        let parecido = |a: (u32, u32)| a.1 == alvo.1 && (a.0 as f64 - alvo.0 as f64).abs() < 0.08 * alvo.0 as f64;
        if parecido(self.gravada) || em_curso.is_some_and(parecido) {
            return;
        }
        if em_curso.is_some() {
            return; // espera a atual terminar e decide de novo
        }
        let (tx, rx) = mpsc::channel();
        let (fonte, partitura) = (fonte.clone(), self.partitura.clone());
        std::thread::spawn(move || {
            let _ = tx.send(notacao::gravar(&fonte, alvo.0, alvo.1, &partitura));
        });
        self.pedido = Some((alvo.0, alvo.1, rx));
    }

    fn receber(&mut self) {
        let Some((largura, escala, rx)) = &self.pedido else { return };
        let Ok(resultado) = rx.try_recv() else { return };
        self.gravada = (*largura, *escala);
        self.pedido = None;
        match resultado {
            Ok(n) => {
                self.status = format!(
                    "{} sistemas, {} de {} notas ligadas ao piano roll",
                    n.sistemas.len(),
                    n.casadas,
                    n.notas.len()
                );
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

    /// Chamado a cada quadro: `agora` em semínimas (None antes de tocar).
    pub fn atualizar(&mut self, ui: &PianoRoll, agora: Option<f64>, tocando: bool, selecionada: Option<usize>) {
        let fator = ui.window().scale_factor();
        if (fator - self.fator).abs() > 1e-3 {
            self.fator = fator;
            self.cache.clear();
        }
        let modo = ui.get_modo_partitura();
        if modo != 0 {
            self.diagramar_se_preciso(ui.get_partitura_largura() as u32);
        }
        self.receber();
        ui.set_status_partitura(self.status.clone().into());
        ui.set_zoom_partitura(format!("{}%", self.escala * 100 / ESCALA_PADRAO).into());
        let Some(n) = &self.notacao else {
            ui.set_sistemas(ModelRc::new(VecModel::from(Vec::<SistemaVis>::new())));
            ui.set_marcas(ModelRc::new(VecModel::from(Vec::<MarcaVis>::new())));
            return;
        };
        let altura_vista = ui.get_partitura_altura() as f64;
        let largura_vista = ui.get_partitura_largura() as f64;
        let topos = self.topos();

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
        if modo == 1 {
            self.rolagem = topos[k];
        }

        // o que soa agora e o que está selecionado (só refaz o modelo se mudou)
        let estados: Vec<(usize, i32)> = n
            .notas
            .iter()
            .enumerate()
            .filter_map(|(j, g)| {
                let i = g.indice?;
                let nota = &self.partitura.notas[i];
                let soando = agora.is_some_and(|t| g.tempo <= t + 1e-9 && t < nota.inicio + nota.duracao);
                let estado = if selecionada == Some(i) { 2 } else if soando { 1 } else { 0 };
                (estado != 0).then_some((j, estado))
            })
            .collect();
        let chave = ((self.rolagem * 10.0) as i64, estados.clone(), (largura_vista as u32, altura_vista as u32), self.cache.len().min(1));
        if self.desenhado.as_ref() == Some(&chave) && !self.cache.is_empty() {
            return;
        }

        // sistemas visíveis, centralizados na largura
        let mut sistemas = Vec::new();
        let mut marcas = Vec::new();
        let visiveis: Vec<usize> = (0..n.sistemas.len())
            .filter(|&i| topos[i] + n.sistemas[i].altura >= self.rolagem && topos[i] <= self.rolagem + altura_vista)
            .collect();
        let estado_de: HashMap<usize, i32> = estados.iter().copied().collect();
        let mut desenhos = Vec::new();
        for &i in &visiveis {
            let s = &n.sistemas[i];
            let x0 = ((largura_vista - s.largura) / 2.0).max(0.0);
            let y0 = topos[i] - self.rolagem;
            desenhos.push((i, x0, y0, s.largura, s.altura));
            for (j, g) in n.notas.iter().enumerate().filter(|(_, g)| g.sistema == i) {
                marcas.push(MarcaVis {
                    id: j as i32,
                    x: (x0 + g.x) as f32,
                    y: (y0 + g.y) as f32,
                    w: g.w as f32,
                    h: g.h as f32,
                    estado: estado_de.get(&j).copied().unwrap_or(0),
                });
            }
        }
        for (i, x0, y0, w, h) in desenhos {
            if let Some(imagem) = self.imagem(i) {
                sistemas.push(SistemaVis { imagem, x: x0 as f32, y: y0 as f32, w: w as f32, h: h as f32 });
            }
        }
        ui.set_sistemas(ModelRc::new(VecModel::from(sistemas)));
        ui.set_marcas(ModelRc::new(VecModel::from(marcas)));
        self.desenhado = Some(chave);
    }

    /// Diagrama já, na thread atual (captura sem janela).
    pub fn diagramar_agora(&mut self, largura: u32) {
        if let Some(fonte) = &self.fonte {
            match notacao::gravar(fonte, largura, self.escala, &self.partitura) {
                Ok(n) => {
                    self.status = format!("{} sistemas, {} de {} notas ligadas ao piano roll", n.sistemas.len(), n.casadas, n.notas.len());
                    self.notacao = Some(n);
                }
                Err(e) => self.status = format!("Partitura indisponível: {e}"),
            }
            self.gravada = (largura, self.escala);
            self.cache.clear();
            self.desenhado = None;
        }
    }
}
