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

//! Interface gráfica: piano roll em Slint, ligado ao player e ao motor.
//!
//! O relógio do áudio manda: a cada quadro da tela a interface lê onde o
//! motor está e desenha o playhead ali; os eventos seguem para o motor pelo
//! [`transporte`]. Sem dispositivo de áudio, a interface funciona igual com
//! o relógio do sistema (sem som).

mod acervo;
pub mod linha_do_tempo;
#[cfg(feature = "partitura")]
mod painel;
#[cfg(not(feature = "partitura"))]
#[path = "painel_vazio.rs"]
mod painel;
pub mod transporte;

slint::include_modules!();

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

use crate::audio;
use crate::corda::{self, Corda};
use crate::dedilhado::Mao;
use crate::emocao;
use crate::eventos::{Evento, EventoAgendado, Pedal};
use crate::expressao::Perfil;
use crate::motor::{Estado as EstadoMotor, Motor};
use crate::musicxml;
use crate::partitura::Partitura;
use crate::player::{Execucao, Player};
use linha_do_tempo::{Comparada, Estado, NotaDesenho, Peso, Vista};
use painel::Painel;
use transporte::Transporte;

/// Uma peça pronta para mostrar: o que está escrito e como o player tocou.
pub struct Peca {
    pub partitura: Partitura,
    pub execucao: Execucao,
    /// o MusicXML de onde a partitura veio (para a notação tradicional)
    pub fonte: Option<String>,
}

enum Relogio {
    Audio(Arc<EstadoMotor>),
    Sistema(Instant),
}

impl Relogio {
    fn quadro(&self, taxa: u32) -> u64 {
        match self {
            Relogio::Audio(e) => e.relogio.load(Relaxed),
            Relogio::Sistema(t) => (t.elapsed().as_secs_f64() * taxa as f64) as u64,
        }
    }
}

/// Cria um motor novo (para abrir ou trocar o dispositivo de áudio).
pub type FabricaMotor = Box<dyn Fn() -> Result<Motor, String>>;

struct App {
    peca: Peca,
    /// partitura tradicional, sincronizada com o piano roll
    painel: Painel,
    /// áudio: o stream aberto, como recriar o motor e onde tocar
    saida: Option<audio::Saida>,
    fabrica: Option<FabricaMotor>,
    dispositivo: String,
    latencia_us: u32,
    /// volume em % (100 = padrão)
    volume: f32,
    cordas: Vec<Corda>,
    vista: Vista,
    transporte: Transporte,
    relogio: Relogio,
    fila: Option<rtrb::Producer<EventoAgendado>>,
    taxa: u32,
    selecionada: Option<usize>,
    /// pisada de pedal mostrada no inspetor
    pisada: Option<usize>,
    cor_por_mao: bool,
    modelo_guia: bool,
    /// o player (humanização, pé, interpretação) para as peças abertas
    /// pela interface
    player: Player,
    /// as quatro interpretações da peça, medidas, para comparar
    comparadas: Vec<Comparada>,
    /// clicou numa nota ou num pedal: mostrar a aba Nota
    abrir_nota: bool,
    /// a tela de busca de partituras
    acervo: acervo::TelaAcervo,
    /// o que foi entregue ao Slint da última vez (para não refazer à toa)
    desenhado: Option<(Vista, Vec<NotaDesenho>, Option<usize>, bool)>,
    /// vigia do áudio: último pulso do motor e quando mudou
    pulso: (u64, Instant),
    mensagem: String,
}

fn cor((r, g, b): (u8, u8, u8)) -> Color {
    Color::from_rgb_u8(r, g, b)
}

/// Cores das curvas de valência e de ativação na faixa de emoção (as mesmas
/// dos rótulos no .slint) e altura da faixa.
const COR_VALENCIA: (u8, u8, u8) = (0x7f, 0xc8, 0xf8);
const COR_ATIVACAO: (u8, u8, u8) = (0xf2, 0x8c, 0x6a);
const ALTURA_EMOCAO: f64 = 44.0;
/// Cores das interpretações (as mesmas do global Interpretacoes no .slint).
const CORES_PERFIL: [(u8, u8, u8); 4] = [(0xa8, 0xac, 0xb4), (0x5f, 0xb3, 0xa1), (0xe8, 0x91, 0x5a), (0x8a, 0x7f, 0xd4)];

fn indice_perfil(perfil: Perfil) -> usize {
    Perfil::TODOS.iter().position(|&p| p == perfil).unwrap_or(0)
}

/// As quatro interpretações da partitura, tocadas pelo mesmo player e
/// medidas (emoção percebida estimada e resumo).
fn comparar(p: &Partitura, player: Player, taxa: u32) -> Vec<Comparada> {
    Perfil::TODOS
        .iter()
        .map(|&perfil| {
            let e = Player { perfil, ..player }.tocar(p, taxa);
            let trajetoria = emocao::trajetoria(p, &e);
            let tempos = trajetoria.iter().map(|u| e.relogio.tempo_em(u.s)).collect();
            let resumo = emocao::resumir(p, &e, &trajetoria);
            Comparada { perfil, trajetoria, tempos, resumo }
        })
        .collect()
}

impl App {
    fn quadro(&self) -> u64 {
        self.relogio.quadro(self.taxa)
    }

    /// Posição atual em semínimas (None antes de começar).
    fn agora_tempo(&self) -> Option<f64> {
        let s = self.transporte.agora_s(self.quadro());
        // pelo relógio da execução: inclui o ritardando do fim
        (self.transporte.tocando() || s > 0.0).then(|| self.peca.execucao.relogio.tempo_em(s))
    }

    /// Abre (ou troca) o dispositivo de áudio com um motor novo, mantendo
    /// volume e modelo de corda. A reprodução pausa; Tocar continua dali.
    fn conectar(&mut self, dispositivo: &str) -> Result<(), String> {
        self.com_transporte(|t, q, envia| t.pausar(q, envia));
        self.saida = None; // fecha o stream anterior antes de abrir outro
        self.fila = None;
        self.relogio = Relogio::Sistema(Instant::now());
        let fabrica = self.fabrica.as_ref().ok_or("sem áudio")?;
        let motor = fabrica()?;
        let estado = motor.estado();
        let modelo_guia = estado.modelo_guia.load(Relaxed);
        let (mut prod, cons) = rtrb::RingBuffer::new(4096);
        let saida = audio::abrir(dispositivo, self.taxa, self.latencia_us, motor, cons)?;
        let _ = prod.push(EventoAgendado::agora(Evento::Volume { valor: self.volume / 100.0 }));
        if modelo_guia != self.modelo_guia {
            let _ = prod.push(EventoAgendado::agora(Evento::AlternarModelo));
        }
        self.saida = Some(saida);
        self.fila = Some(prod);
        self.relogio = Relogio::Audio(estado);
        self.pulso = (u64::MAX, Instant::now());
        self.dispositivo = dispositivo.to_string();
        Ok(())
    }

    /// Usa o transporte com a fila do motor (sem áudio, os eventos somem).
    fn com_transporte(&mut self, f: impl FnOnce(&mut Transporte, u64, &mut dyn FnMut(EventoAgendado) -> bool)) {
        let q = self.quadro();
        let fila = &mut self.fila;
        let mut envia = |e| fila.as_mut().is_none_or(|f| f.push(e).is_ok());
        f(&mut self.transporte, q, &mut envia);
    }

    fn tocar(&mut self) {
        self.com_transporte(|t, q, envia| t.tocar(q, envia));
        self.pulso = (self.pulso.0, Instant::now());
    }

    fn legenda(&self) -> Vec<LegendaVis> {
        if self.cor_por_mao {
            return [("Mão esquerda", Some(Mao::Esquerda)), ("Mão direita", Some(Mao::Direita))]
                .into_iter()
                .map(|(t, m)| LegendaVis { texto: t.into(), cor: cor(linha_do_tempo::cor_mao(m)) })
                .collect();
        }
        let mut vozes: Vec<u8> = self.peca.partitura.notas.iter().map(|n| n.voz).collect();
        vozes.sort_unstable();
        vozes.dedup();
        // muitas vozes: rótulos curtos para caber na barra
        let curto = vozes.len() > 3;
        vozes
            .into_iter()
            .take(8)
            .map(|v| LegendaVis {
                texto: if curto { format!("V{v}") } else { format!("Voz {v}") }.into(),
                cor: cor(linha_do_tempo::cor_voz(v)),
            })
            .collect()
    }

    fn cor_da_nota(&self, n: &NotaDesenho) -> Color {
        cor(if self.cor_por_mao { linha_do_tempo::cor_mao(n.mao) } else { linha_do_tempo::cor_voz(n.voz) })
    }

    /// Plano emocional, botões e resumo da interpretação escolhida.
    fn entregar_interpretacao(&self, ui: &PianoRoll) {
        let atual = indice_perfil(self.player.perfil);
        ui.set_interpretacao(atual as i32);
        let curvas: Vec<CurvaVis> = self
            .comparadas
            .iter()
            .enumerate()
            // a escolhida por último, por cima das outras
            .filter(|(i, _)| *i != atual)
            .chain(self.comparadas.iter().enumerate().filter(|(i, _)| *i == atual))
            .map(|(i, c)| CurvaVis { comandos: linha_do_tempo::curva_no_plano(c).into(), cor: cor(CORES_PERFIL[i]), destaque: i == atual })
            .collect();
        ui.set_curvas_plano(ModelRc::new(VecModel::from(curvas)));
        if let (Some(c), Some(n)) = (self.comparadas.get(atual), self.comparadas.first()) {
            ui.set_resumo_interpretacao(linha_do_tempo::descrever_comparada(c, n).into());
        }
    }

    /// Troca a interpretação sem perder o lugar: o player toca de novo com o
    /// outro perfil e a execução continua do mesmo ponto da partitura.
    fn escolher_interpretacao(&mut self, ui: &PianoRoll, perfil: Perfil) {
        if perfil == self.player.perfil {
            return;
        }
        let agora = self.agora_tempo();
        let tocando = self.transporte.tocando();
        self.com_transporte(|t, q, envia| t.parar(q, envia));
        self.player.perfil = perfil;
        self.peca.execucao = self.player.tocar(&self.peca.partitura, self.taxa);
        let r = &self.peca.execucao.roteiro;
        self.transporte = Transporte::new(r.eventos.clone(), r.duracao, self.taxa);
        if let Some(t) = agora {
            let s = self.peca.execucao.relogio.segundos(t);
            self.com_transporte(|tr, q, envia| tr.saltar(s, q, envia));
        }
        if tocando {
            self.tocar();
        }
        self.mensagem = resumo(&self.peca);
        self.desenhado = None;
        self.entregar_interpretacao(ui);
    }

    /// Chamado a cada quadro da tela.
    fn atualizar(&mut self, ui: &PianoRoll) {
        if std::mem::take(&mut self.abrir_nota) {
            ui.set_aba(1);
        }
        self.acervo.atualizar(ui);
        // vigia: tocando e o motor parou de pedir amostras
        if let Relogio::Audio(e) = &self.relogio {
            let p = e.pulso.load(Relaxed);
            if p != self.pulso.0 {
                self.pulso = (p, Instant::now());
            } else if self.transporte.tocando() && self.pulso.1.elapsed() > Duration::from_secs(3) {
                self.com_transporte(|t, q, envia| t.pausar(q, envia));
                self.mensagem = "O áudio parou de responder há 3 s (servidor de som travado?). \
                                 Reiniciar: systemctl --user restart pipewire pipewire-pulse wireplumber"
                    .into();
            }
        }
        self.com_transporte(|t, q, envia| t.alimentar(q, envia));

        self.vista.redimensionar(ui.get_roll_largura() as f64, ui.get_roll_altura() as f64);
        let agora = self.agora_tempo();
        if let (true, Some(t)) = (self.transporte.tocando(), agora) {
            self.vista.seguir(t);
        }

        let p = &self.peca.partitura;
        let tocadas = linha_do_tempo::tocadas_por_nota(p, &self.peca.execucao.notas);
        let notas = linha_do_tempo::notas_visiveis(&self.vista, p, &tocadas, agora);
        let chave = (self.vista.clone(), notas.clone(), self.selecionada, self.cor_por_mao);
        if self.desenhado.as_ref() != Some(&chave) {
            self.entregar(ui, &notas, agora);
            self.desenhado = Some(chave);
        }

        let tempo = agora.unwrap_or(0.0);
        let x = self.vista.x(tempo);
        ui.set_playhead_x(x as f32);
        ui.set_playhead_visivel(agora.is_some() && x >= 0.0 && x <= self.vista.largura);
        let (compasso, no_compasso) = p.compasso_e_tempo(tempo);
        ui.set_posicao(format!("Compasso {compasso}, tempo {no_compasso:.2}").replace('.', ",").into());
        ui.set_bpm(format!("{:.0}", p.bpm_em(tempo)).into());
        let (tempos, unidade) = p.formula_em(tempo);
        ui.set_formula(format!("{tempos}/{unidade}").into());
        ui.set_zoom(format!("{:.0}%", self.vista.zoom_percentual()).into());
        ui.set_tocando(self.transporte.tocando());
        // o pé agora: com áudio, o que o instrumento está fazendo; sem, o plano
        let pe = match &self.relogio {
            Relogio::Audio(e) => [Pedal::Sustain, Pedal::Sostenuto, Pedal::UnaCorda].map(|p| e.pedal(p)),
            Relogio::Sistema(_) if agora.is_some() => linha_do_tempo::pedais_em(&self.peca.execucao.pedais, self.transporte.agora_s(self.quadro())),
            Relogio::Sistema(_) => [0.0; 3],
        };
        ui.set_pe_agora(ModelRc::new(VecModel::from(pe.to_vec())));
        // a emoção estimada agora, no plano
        let ponto = agora.and_then(|_| {
            let c = self.comparadas.get(indice_perfil(self.player.perfil))?;
            emocao::em(&c.trajetoria, self.transporte.agora_s(self.quadro()))
        });
        ui.set_agora_visivel(ponto.is_some());
        if let Some(u) = ponto {
            ui.set_agora_valencia(u.valencia);
            ui.set_agora_ativacao(u.ativacao);
        }
        ui.set_mensagem(self.mensagem.clone().into());
        self.painel.atualizar(ui, &self.vista, agora, self.transporte.tocando(), self.selecionada);
    }

    /// Entrega ao Slint o que está visível.
    fn entregar(&self, ui: &PianoRoll, notas: &[NotaDesenho], agora: Option<f64>) {
        let p = &self.peca.partitura;
        let notas_vis: Vec<NotaVis> = notas
            .iter()
            .map(|n| NotaVis {
                id: n.indice as i32,
                x: n.x as f32,
                y: n.y as f32,
                w: n.w as f32,
                h: n.h as f32,
                cor: self.cor_da_nota(n),
                rotulo: n.dedo.map_or(SharedString::new(), |d| d.to_string().into()),
                estado: match n.estado {
                    Estado::Futura => 0,
                    Estado::Soando => 1,
                    Estado::Tocada => 2,
                },
                selecionada: self.selecionada == Some(n.indice),
            })
            .collect();
        let barras: Vec<BarraVis> = notas
            .iter()
            .map(|n| BarraVis {
                x: n.x as f32,
                nivel: n.vel,
                cor: self.cor_da_nota(n),
                selecionada: self.selecionada == Some(n.indice),
            })
            .collect();
        let soando: Vec<u8> = notas.iter().filter(|n| n.estado == Estado::Soando).map(|n| p.notas[n.indice].tecla).collect();
        let teclas: Vec<TeclaVis> = linha_do_tempo::teclas_visiveis(&self.vista)
            .into_iter()
            .map(|t| TeclaVis {
                y: t.y as f32,
                h: t.h as f32,
                preta: t.preta,
                rotulo: t.rotulo.into(),
                soando: agora.is_some() && soando.contains(&t.tecla),
                divisao: t.divisao,
            })
            .collect();
        let g = linha_do_tempo::grade(&self.vista, p);
        let linhas: Vec<LinhaVis> = g
            .linhas
            .iter()
            .map(|&(x, peso)| LinhaVis {
                x: x as f32,
                peso: match peso {
                    Peso::Subdivisao => 0,
                    Peso::Tempo => 1,
                    Peso::Compasso => 2,
                },
            })
            .collect();
        let rotulos: Vec<RotuloVis> = g.rotulos.into_iter().map(|(x, t)| RotuloVis { x: x as f32, texto: t.into() }).collect();
        // como o pé pedalou (marcações da partitura ou pedal automático)
        let e = &self.peca.execucao;
        let pedais: Vec<FaixaVis> = linha_do_tempo::pedais_visiveis(&self.vista, &e.pedais, &e.relogio)
            .into_iter()
            .map(|d| FaixaVis {
                id: d.indice as i32,
                x: d.x as f32,
                w: d.w as f32,
                linha: d.linha as i32,
                nivel: d.valor,
                selecionada: self.pisada == Some(d.indice),
            })
            .collect();

        ui.set_notas(ModelRc::new(VecModel::from(notas_vis)));
        ui.set_barras(ModelRc::new(VecModel::from(barras)));
        ui.set_teclas(ModelRc::new(VecModel::from(teclas)));
        ui.set_linhas(ModelRc::new(VecModel::from(linhas)));
        ui.set_rotulos(ModelRc::new(VecModel::from(rotulos)));
        ui.set_pedais(ModelRc::new(VecModel::from(pedais)));
        // emoção percebida: a interpretação escolhida e, apagada, a neutra
        let atual = indice_perfil(self.player.perfil);
        let mut curvas = Vec::new();
        for (i, c) in self.comparadas.iter().enumerate().filter(|(i, _)| *i == 0 || *i == atual) {
            let dimensoes: [(fn(&emocao::Ponto) -> f32, _); 2] = [(|u| u.valencia, COR_VALENCIA), (|u| u.ativacao, COR_ATIVACAO)];
            for (valor, tom) in dimensoes {
                let comandos = linha_do_tempo::curva_no_tempo(&self.vista, c, valor, ALTURA_EMOCAO);
                if !comandos.is_empty() {
                    curvas.push(CurvaVis { comandos: comandos.into(), cor: cor(tom), destaque: i == atual });
                }
            }
        }
        ui.set_curvas_emocao(ModelRc::new(VecModel::from(curvas)));
        ui.set_grade(g.resolucao.into());
        ui.set_legenda(ModelRc::new(VecModel::from(self.legenda())));
        ui.set_modo_cor(if self.cor_por_mao { "Cores: mãos" } else { "Cores: vozes" }.into());
        ui.set_modelo(if self.modelo_guia { "Corda: guia de onda" } else { "Corda: modal" }.into());
        let info = match (self.selecionada, self.pisada) {
            (_, Some(i)) => linha_do_tempo::descrever_pisada(p, &self.peca.execucao.pedais[i], &self.peca.execucao.relogio),
            (Some(i), None) => {
                let tocadas = linha_do_tempo::tocadas_por_nota(p, &self.peca.execucao.notas);
                linha_do_tempo::descrever(p, i, tocadas[i], &self.cordas)
            }
            (None, None) => "Clique numa nota ou num pedal para ver os detalhes.".into(),
        };
        ui.set_info(info.into());
    }

    fn selecionar(&mut self, i: Option<usize>) {
        self.selecionada = i;
        self.pisada = None;
        self.desenhado = None;
    }

    /// Troca a peça: o player toca a nova partitura e a vista volta ao começo.
    fn trocar_peca(&mut self, ui: &PianoRoll, partitura: Partitura, fonte: Option<String>) {
        self.com_transporte(|t, q, envia| t.parar(q, envia));
        let execucao = self.player.tocar(&partitura, self.taxa);
        self.comparadas = comparar(&partitura, self.player, self.taxa);
        self.painel = Painel::new(fonte.clone(), partitura.clone());
        self.peca = Peca { partitura, execucao, fonte };
        self.transporte = Transporte::new(self.peca.execucao.roteiro.eventos.clone(), self.peca.execucao.roteiro.duracao, self.taxa);
        self.vista = enquadrar(ui, &self.peca.partitura);
        self.selecionada = None;
        self.pisada = None;
        self.desenhado = None;
        self.mensagem = resumo(&self.peca);
        ui.set_titulo(self.peca.partitura.titulo.clone().into());
        self.entregar_interpretacao(ui);
    }

    /// Abre um MusicXML (do diálogo ou do acervo) no lugar da peça atual.
    fn abrir_arquivo(&mut self, ui: &PianoRoll, arquivo: &Path, tocar: bool) -> Result<(), String> {
        let p = musicxml::ler(arquivo)?;
        if p.notas.is_empty() {
            return Err(format!("{}: nenhuma nota encontrada.", arquivo.display()));
        }
        self.trocar_peca(ui, p, musicxml::ler_texto(arquivo).ok());
        if tocar {
            self.tocar();
        }
        Ok(())
    }
}

/// Vista inicial: começo da peça, teclas usadas no meio da tela.
fn enquadrar(ui: &PianoRoll, p: &Partitura) -> Vista {
    let mut vista = Vista::default();
    vista.redimensionar(ui.get_roll_largura() as f64, ui.get_roll_altura() as f64);
    let (min, max) = p.notas.iter().fold((87u8, 0u8), |(a, b), n| (a.min(n.tecla), b.max(n.tecla)));
    vista.centralizar(min.min(max), max);
    vista
}

fn resumo(peca: &Peca) -> String {
    let avisos = &peca.execucao.avisos;
    let mut s = format!("{} notas, {} avisos do player.", peca.partitura.notas.len(), avisos.len());
    for a in avisos.iter().take(2) {
        s += &format!("\n- {a}");
    }
    if avisos.len() > 2 {
        s += &format!("\n(e mais {})", avisos.len() - 2);
    }
    s + "\nRoda: rolar | Shift+roda: tempo | Ctrl+roda: zoom | clique na régua: ir para"
}

/// Diálogo do sistema para escolher um MusicXML.
fn escolher_musicxml() -> Option<std::path::PathBuf> {
    let mut dialogo = rfd::FileDialog::new()
        .set_title("Abrir partitura MusicXML")
        .add_filter("MusicXML", &["musicxml", "xml", "mxl"]);
    if let Ok(pasta) = std::env::current_dir() {
        let exemplos = pasta.join("exemplos");
        dialogo = dialogo.set_directory(if exemplos.is_dir() { exemplos } else { pasta });
    }
    dialogo.pick_file()
}

/// Monta a interface, ainda sem áudio (ver [`App::conectar`]).
fn montar(ui: &PianoRoll, peca: Peca, taxa: u32, modelo_guia: bool, player: Player) -> Rc<RefCell<App>> {
    let transporte = Transporte::new(peca.execucao.roteiro.eventos.clone(), peca.execucao.roteiro.duracao, taxa);
    let vista = enquadrar(ui, &peca.partitura);
    let mensagem = resumo(&peca);
    ui.set_titulo(peca.partitura.titulo.clone().into());
    let painel = Painel::new(peca.fonte.clone(), peca.partitura.clone());
    let comparadas = comparar(&peca.partitura, player, taxa);
    let app = Rc::new(RefCell::new(App {
        cordas: corda::cordas(1.0),
        painel,
        peca,
        saida: None,
        fabrica: None,
        dispositivo: String::new(),
        latencia_us: 30000,
        volume: 100.0,
        vista,
        transporte,
        relogio: Relogio::Sistema(Instant::now()),
        fila: None,
        taxa,
        selecionada: None,
        pisada: None,
        cor_por_mao: false,
        modelo_guia,
        player,
        comparadas,
        abrir_nota: false,
        acervo: acervo::TelaAcervo::new(),
        desenhado: None,
        pulso: (u64::MAX, Instant::now()),
        mensagem,
    }));

    app.borrow().entregar_interpretacao(ui);
    let fraco = ui.as_weak();
    macro_rules! ligar {
        ($metodo:ident, |$a:ident $(, $arg:ident)*| $corpo:expr) => {{
            let app = app.clone();
            let fraco = fraco.clone();
            ui.$metodo(move |$($arg),*| {
                let mut $a = app.borrow_mut();
                $corpo;
                if let Some(ui) = fraco.upgrade() {
                    $a.atualizar(&ui);
                }
            });
        }};
    }
    ligar!(on_tocar, |a| a.tocar());
    {
        // o diálogo bloqueia até o usuário escolher: pausa antes, sem segurar
        // o estado durante a espera
        let app = app.clone();
        let fraco = fraco.clone();
        ui.on_abrir(move || {
            app.borrow_mut().com_transporte(|t, q, envia| t.pausar(q, envia));
            let Some(arquivo) = escolher_musicxml() else { return };
            let Some(ui) = fraco.upgrade() else { return };
            let mut a = app.borrow_mut();
            if let Err(e) = a.abrir_arquivo(&ui, &arquivo, true) {
                a.mensagem = format!("Não foi possível abrir: {e}");
            }
            a.desenhado = None;
            a.atualizar(&ui);
        });
    }
    // acervo: a tela de busca
    macro_rules! ligar_acervo {
        ($metodo:ident, |$a:ident, $ui:ident $(, $arg:ident)*| $corpo:expr) => {{
            let app = app.clone();
            let fraco = fraco.clone();
            ui.$metodo(move |$($arg),*| {
                let Some($ui) = fraco.upgrade() else { return };
                let mut $a = app.borrow_mut();
                $corpo;
            });
        }};
    }
    ligar_acervo!(on_abrir_acervo, |a, ui| a.acervo.mostrar(&ui));
    ligar_acervo!(on_acervo_buscar, |a, ui| a.acervo.buscar(&ui));
    ligar_acervo!(on_acervo_reindexar, |a, ui| a.acervo.reindexar(&ui));
    ligar_acervo!(on_acervo_abrir, |a, ui, indice, tocar| {
        let Some(arquivo) = a.acervo.caminho(indice as usize) else { return };
        match a.abrir_arquivo(&ui, &arquivo, tocar) {
            Ok(()) => {
                a.acervo.abriu(&arquivo);
                ui.set_tela(0);
                a.desenhado = None;
                a.atualizar(&ui);
            }
            Err(e) => ui.set_acervo_titulo_lista(format!("Não foi possível abrir: {e}").into()),
        }
    });
    {
        // como no Abrir: o diálogo bloqueia, sem segurar o estado
        let app = app.clone();
        let fraco = fraco.clone();
        ui.on_acervo_escolher_pasta(move || {
            let Some(pasta) = rfd::FileDialog::new().set_title("Pasta do acervo de partituras").pick_folder() else { return };
            let Some(ui) = fraco.upgrade() else { return };
            app.borrow_mut().acervo.trocar_pasta(&ui, pasta);
        });
    }
    ligar!(on_pausar, |a| a.com_transporte(|t, q, envia| t.pausar(q, envia)));
    ligar!(on_parar, |a| a.com_transporte(|t, q, envia| t.parar(q, envia)));
    ligar!(on_alternar_cor, |a| {
        a.cor_por_mao = !a.cor_por_mao;
    });
    ligar!(on_alternar_modelo, |a| {
        a.modelo_guia = !a.modelo_guia;
        if let Some(f) = a.fila.as_mut() {
            let _ = f.push(EventoAgendado::agora(Evento::AlternarModelo));
        }
        a.desenhado = None;
    });
    // piano roll: seleciona e mostra a nota na partitura
    ligar!(on_clicar_nota, |a, id| {
        a.selecionar(Some(id as usize));
        a.abrir_nota = true;
        a.painel.mostrar(Some(id as usize));
    });
    // partitura: seleciona e leva o playhead (e o roll) até a nota
    ligar!(on_clicar_partitura, |a, id| {
        if let Some((indice, tempo)) = a.painel.nota_da_marca(id as usize) {
            a.selecionar(indice);
            let s = a.peca.execucao.relogio.segundos(tempo);
            a.com_transporte(|t, q, envia| t.saltar(s, q, envia));
            a.vista.seguir(tempo);
        }
    });
    ligar!(on_rolar_partitura, |a, dy| a.painel.rolar(dy as f64));
    ligar!(on_zoom_partitura_mais, |a| a.painel.zoom(5));
    ligar!(on_zoom_partitura_menos, |a| a.painel.zoom(-5));
    ligar!(on_mudou_partitura, |a| a.desenhado = None);
    // botões de zoom: 25% por clique, com o centro da tela parado
    ligar!(on_zoom_mais, |a| {
        let meio = a.vista.largura / 2.0;
        a.vista.zoom_em(1.25, meio);
    });
    ligar!(on_zoom_menos, |a| {
        let meio = a.vista.largura / 2.0;
        a.vista.zoom_em(0.8, meio);
    });
    ligar!(on_mudar_volume, |a, v| {
        a.volume = v;
        if let Some(f) = a.fila.as_mut() {
            let _ = f.push(EventoAgendado::agora(Evento::Volume { valor: v / 100.0 }));
        }
    });
    ligar!(on_escolher_dispositivo, |a, nome| {
        a.mensagem = match a.conectar(&nome) {
            Ok(()) => format!("Saída: {nome}. Tocar continua de onde parou."),
            Err(e) => format!("Não foi possível usar {nome}: {e}. Sem som até escolher outra saída."),
        };
    });
    ligar!(on_clicar_fundo, |a| a.selecionar(None));
    ligar!(on_clicar_pedal, |a, id| {
        a.selecionar(None);
        a.pisada = Some(id as usize);
        a.abrir_nota = true;
    });
    {
        let app = app.clone();
        let fraco = fraco.clone();
        ui.on_escolher_interpretacao(move |i| {
            let Some(ui) = fraco.upgrade() else { return };
            let mut a = app.borrow_mut();
            a.escolher_interpretacao(&ui, Perfil::TODOS[(i as usize).min(3)]);
            a.atualizar(&ui);
        });
    }
    ligar!(on_clicar_regua, |a, x| {
        let tempo = a.vista.tempo_de_x(x as f64).max(0.0);
        let s = a.peca.execucao.relogio.segundos(tempo);
        a.com_transporte(|t, q, envia| t.saltar(s, q, envia));
    });
    ligar!(on_rolar, |a, dx, dy, ctrl, shift, x, y| {
        let (dx, dy) = (dx as f64, dy as f64);
        if ctrl && shift {
            a.vista.zoom_vertical_em((dy * 0.003).exp(), y as f64);
        } else if ctrl {
            a.vista.zoom_em((dy * 0.003).exp(), x as f64);
        } else if shift {
            a.vista.rolar(-(dy + dx), 0.0);
        } else {
            a.vista.rolar(-dx, -dy);
        }
    });
    app
}

/// Abre a janela e toca pelo dispositivo de áudio (sem áudio, só mostra).
/// `modelo_guia`: o modelo de corda com que a fábrica cria os motores.
#[allow(clippy::too_many_arguments)]
pub fn rodar(peca: Peca, fabrica: FabricaMotor, modelo_guia: bool, taxa: u32, dispositivo: &str, latencia_us: u32, player: Player) -> Result<(), String> {
    let ui = PianoRoll::new().map_err(|e| e.to_string())?;
    let app = montar(&ui, peca, taxa, modelo_guia, player);
    {
        let mut a = app.borrow_mut();
        a.fabrica = Some(fabrica);
        a.latencia_us = latencia_us;
        if let Err(e) = a.conectar(dispositivo) {
            a.mensagem = format!("Sem áudio ({e}): só a visualização.\n{}", a.mensagem);
        }
        // dispositivos de saída, com o atual selecionado
        let mut nomes = audio::listar();
        if !nomes.iter().any(|n| n == dispositivo) {
            nomes.insert(0, dispositivo.to_string());
        }
        let atual = nomes.iter().position(|n| n == dispositivo).unwrap_or(0);
        ui.set_dispositivos(ModelRc::new(VecModel::from(nomes.into_iter().map(SharedString::from).collect::<Vec<_>>())));
        ui.set_dispositivo_atual(atual as i32);
    }

    let fraco = ui.as_weak();
    let quadros = slint::Timer::default();
    let app_timer = app.clone();
    quadros.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
        if let Some(ui) = fraco.upgrade() {
            app_timer.borrow_mut().atualizar(&ui);
        }
    });
    let r = ui.run().map_err(|e| e.to_string());
    // para o som e fecha o stream antes de sair
    let mut a = app.borrow_mut();
    a.com_transporte(|t, q, envia| t.parar(q, envia));
    a.saida = None;
    r
}

// ---------- captura em PNG (sem janela) ----------

use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, TargetPixel};

#[derive(Clone, Copy, Default)]
struct Pixel([u8; 4]);

impl TargetPixel for Pixel {
    fn blend(&mut self, c: PremultipliedRgbaColor) {
        let a = 255 - c.alpha as u16;
        let mistura = |fundo: u8, cor: u8| (cor as u16 + fundo as u16 * a / 255) as u8;
        self.0 = [mistura(self.0[0], c.red), mistura(self.0[1], c.green), mistura(self.0[2], c.blue), 255];
    }

    fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Pixel([r, g, b, 255])
    }
}

struct PlataformaSemTela(Rc<MinimalSoftwareWindow>);

impl slint::platform::Platform for PlataformaSemTela {
    fn create_window_adapter(&self) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

/// Desenha a interface sem abrir janela e salva em PNG, com o playhead em
/// `tempo` (semínimas) e a nota que soa ali selecionada.
/// `modo_partitura`: 0 oculta, 1 compacta, 2 expandida, 3 tela cheia.
/// `acervo`: mostra a tela Acervo desta pasta, com esta busca.
#[allow(clippy::too_many_arguments)]
pub fn capturar(
    peca: Peca,
    player: Player,
    caminho: &Path,
    tempo: Option<f64>,
    modo_partitura: i32,
    alinhado: bool,
    acervo: Option<(&Path, &str)>,
    largura: u32,
    altura: u32,
) -> Result<(), String> {
    let janela = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(PlataformaSemTela(janela.clone()))).map_err(|e| e.to_string())?;
    janela.set_size(slint::PhysicalSize::new(largura, altura));
    let ui = PianoRoll::new().map_err(|e| e.to_string())?;
    ui.show().map_err(|e| e.to_string())?;
    ui.set_modo_partitura(modo_partitura);
    ui.set_alinhado(alinhado);

    let app = montar(&ui, peca, 48000, false, player);
    ui.set_dispositivos(ModelRc::new(VecModel::from(vec![SharedString::from("default")])));
    {
        let mut a = app.borrow_mut();
        if let Some(t) = tempo {
            let s = a.peca.execucao.relogio.segundos(t);
            a.com_transporte(|tr, q, envia| tr.saltar(s, q, envia));
            // a posição efetiva (o transporte limita ao fim da peça)
            let t = a.agora_tempo().unwrap_or(0.0);
            a.vista.seguir(t);
            let soando = a.peca.partitura.notas.iter().position(|n| n.inicio <= t && t < n.inicio + n.duracao);
            a.selecionar(soando);
        }
        a.atualizar(&ui);
        // a vista só sabe o tamanho do roll depois do primeiro desenho
        a.atualizar(&ui);
        // a partitura é diagramada aqui mesmo, sem thread
        let largura = ui.get_partitura_largura() as u32;
        a.painel.diagramar_agora(largura, ui.get_alinhado());
        a.atualizar(&ui);
        if let Some((pasta, busca)) = acervo {
            a.acervo = acervo::TelaAcervo::pronta(crate::acervo::Acervo::abrir(pasta, &Default::default(), false)?);
            ui.set_acervo_consulta(busca.into());
            a.acervo.mostrar(&ui);
        }
    }

    let mut buf = vec![Pixel::default(); (largura * altura) as usize];
    janela.request_redraw();
    janela.draw_if_needed(|r| {
        r.render(&mut buf, largura as usize);
    });
    let arquivo = std::fs::File::create(caminho).map_err(|e| format!("{}: {e}", caminho.display()))?;
    let mut png = png::Encoder::new(std::io::BufWriter::new(arquivo), largura, altura);
    png.set_color(png::ColorType::Rgba);
    png.set_depth(png::BitDepth::Eight);
    let dados: Vec<u8> = buf.iter().flat_map(|p| p.0).collect();
    png.write_header().and_then(|mut w| w.write_image_data(&dados)).map_err(|e| e.to_string())?;
    Ok(())
}
