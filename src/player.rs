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

//! Player: o pianista. Lê uma [`Partitura`] e decide como um humano a
//! tocaria, com duas mãos de cinco dedos e (por enquanto de forma simples)
//! um pé nos pedais. Devolve um [`Roteiro`] de eventos para o instrumento.
//!
//! O que o player faz:
//!
//! 1. mãos: pauta de cima para a mão direita, de baixo para a esquerda; uma
//!    mão toca no máximo 5 notas de uma vez, e o que sobra passa para a
//!    outra mão se ela estiver livre naquele instante (senão é descartado,
//!    com aviso);
//! 2. dedos: escolhidos pelo modelo ergonômico de [`crate::dedilhado`];
//! 3. corpo: acorde que não cabe na mão é arpejado; um dedo só abaixa outra
//!    tecla depois de soltar a anterior; uma tecla só é tocada de novo
//!    depois de solta;
//! 4. tempo e força: semínimas viram segundos pelo andamento; a humanização
//!    (0 = exato) adiciona pequenas variações de tempo e de força e destaca
//!    a nota mais aguda da mão direita (normalmente a melodia);
//! 5. pé: executa as marcações dos três pedais da partitura e, para o
//!    sustain e o una corda que ela não marca, decide sozinho: troca o
//!    sustain quando a harmonia muda e solta nos silêncios (abaixando um
//!    pouco depois do tempo, pedal sincopado); usa o una corda nos trechos
//!    em pp ou mais suave (abaixado um pouco antes das notas); o sostenuto
//!    só vem da partitura;
//! 6. articulações escritas: staccato e staccatissimo encurtam, tenuto liga,
//!    fermata segura mais, acento toca mais forte; o dedilhado escrito é
//!    seguido sempre que fisicamente possível;
//! 7. interpretação: a melodia sai um pouco antes do acompanhamento, o
//!    primeiro tempo do compasso é mais forte e o contratempo mais leve, e a
//!    peça desacelera nos dois últimos compassos (ritardando).

use std::collections::BTreeMap;

use crate::corda;
use crate::dedilhado::{self, Mao};
use crate::eventos::{Evento, Pedal};
use crate::partitura::{Partitura, Pauta};
use crate::roteiro::Roteiro;

/// Notas que uma mão abaixa ao mesmo tempo, no máximo.
const DEDOS: usize = 5;
/// Intervalo entre as notas de um acorde arpejado (s).
const ARPEJO_S: f64 = 0.035;
/// Folga para soltar uma tecla ou dedo antes de reutilizá-lo (s).
const FOLGA_S: f64 = 0.008;
/// O pé abaixa o pedal este tempo depois do início marcado (s).
const ATRASO_PEDAL_S: f64 = 0.06;
/// O una corda desce este tempo antes das notas (a mecânica precisa estar
/// deslocada quando o martelo sai) (s).
const ANTECIPA_UNA_CORDA_S: f64 = 0.04;
/// Dinâmica até a qual o pé automático usa o una corda (pp).
const DINAMICA_UNA_CORDA: f32 = 0.25;
/// Tudo começa um pouco depois de zero, para a humanização poder adiantar.
pub const INICIO_S: f64 = 0.05;

/// Como o pé decide os pedais.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pe {
    /// as marcações da partitura; num pedal que ela não marca, decide sozinho
    #[default]
    Auto,
    /// só as marcações da partitura
    Partitura,
    /// decide sozinho, ignorando as marcações
    Automatico,
    /// nunca pedala
    Nenhum,
}

#[derive(Debug, Clone, Copy)]
pub struct Player {
    /// 0 = exato e repetível; 1 = variações de um pianista humano
    pub humanizacao: f32,
    pub semente: u32,
    /// fração da duração escrita em que a tecla fica abaixada
    pub legato: f64,
    pub pe: Pe,
}

impl Default for Player {
    fn default() -> Self {
        Player { humanizacao: 0.5, semente: 1, legato: 0.95, pe: Pe::Auto }
    }
}

/// Quando cada posição da partitura é tocada: o andamento escrito mais o
/// ritardando do fim. Converte nos dois sentidos (a interface usa para pôr o
/// playhead onde o som está).
#[derive(Debug, Clone, Default)]
pub struct RelogioExecucao {
    /// só os andamentos da partitura
    andamentos: Partitura,
    /// ritardando: (início em semínimas, extensão, quanto desacelera no fim)
    ritardando: (f64, f64, f64),
}

impl RelogioExecucao {
    /// Instante (s) em que a posição `tempo` (semínimas) é tocada.
    pub fn segundos(&self, tempo: f64) -> f64 {
        let (ini, extensao, a) = self.ritardando;
        let mut s = INICIO_S + self.andamentos.segundos(tempo);
        if a > 0.0 && extensao > 0.0 && tempo > ini {
            // o tempo de cada semínima cresce até (1 + a) no fim: soma da
            // diferença, a*L*u^3/3 até o fim do trecho e linear depois
            let seg_por_seminima = 60.0 / self.andamentos.bpm_em(tempo);
            let u = (tempo - ini) / extensao;
            let extra = if u <= 1.0 { a * extensao * u.powi(3) / 3.0 } else { a * extensao / 3.0 + a * (tempo - ini - extensao) };
            s += extra * seg_por_seminima;
        }
        s
    }

    /// O inverso: posição (semínimas) tocada no instante `s`.
    pub fn tempo_em(&self, s: f64) -> f64 {
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        while self.segundos(hi) < s && hi < 1e7 {
            hi *= 2.0;
        }
        for _ in 0..60 {
            let m = 0.5 * (lo + hi);
            if self.segundos(m) < s { lo = m } else { hi = m }
        }
        0.5 * (lo + hi)
    }
}

/// Uma pisada do pé: qual pedal, quando e quanto.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pisada {
    pub pedal: Pedal,
    /// em semínimas, como escrito (ou decidido)
    pub abaixa: f64,
    pub solta: f64,
    /// instantes reais dos eventos mandados ao instrumento (s)
    pub abaixa_s: f64,
    pub solta_s: f64,
    /// profundidade: 0 = solto, 1 = no fundo
    pub valor: f32,
    /// decidida pelo pé (false = marcada na partitura)
    pub automatica: bool,
}

/// Uma nota como foi tocada.
#[derive(Debug, Clone, PartialEq)]
pub struct NotaTocada {
    pub inicio_s: f64,
    pub fim_s: f64,
    pub tecla: u8,
    pub mao: Mao,
    pub dedo: u8,
    pub vel: f32,
    /// tempo escrito (semínimas), para relatórios
    pub tempo: f64,
    /// índice da nota escrita em `Partitura::notas`
    pub indice: usize,
}

pub struct Execucao {
    pub roteiro: Roteiro,
    pub notas: Vec<NotaTocada>,
    pub avisos: Vec<String>,
    /// como o pé pedalou, os três pedais, em ordem de tempo
    pub pedais: Vec<Pisada>,
    pub relogio: RelogioExecucao,
}

/// gerador simples e repetível
struct Acaso(u32);

impl Acaso {
    fn uniforme(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f64 / 16_777_216.0
    }

    /// aproximadamente normal, média 0 e desvio 1
    fn normal(&mut self) -> f64 {
        (0..3).map(|_| self.uniforme()).sum::<f64>() * 2.0 - 3.0
    }
}

/// chave de agrupamento de um instante (semínimas)
fn instante(tempo: f64) -> i64 {
    (tempo * 1e6).round() as i64
}

fn texto_tempo(tempo: f64) -> String {
    format!("tempo {:.2}", tempo + 1.0).replace('.', ",")
}

impl Player {
    pub fn tocar(&self, p: &Partitura, taxa: u32) -> Execucao {
        let mut avisos = Vec::new();

        // 1. mãos: índices das notas por mão e por instante
        let mao_da_pauta = |pauta| if pauta == Pauta::Superior { Mao::Direita } else { Mao::Esquerda };
        let mut momentos: BTreeMap<(Mao, i64), Vec<usize>> = BTreeMap::new();
        for (i, n) in p.notas.iter().enumerate() {
            momentos.entry((mao_da_pauta(n.pauta), instante(n.inicio))).or_default().push(i);
        }
        redistribuir(p, &mut momentos, &mut avisos);

        // 7. quando cada posição é tocada (com o ritardando do fim)
        let h = self.humanizacao as f64;
        let relogio = relogio_da_execucao(p, h);

        // 2. dedos, mão por mão (seguindo o dedo escrito quando houver)
        let mut tocadas = Vec::new();
        let mut acaso = Acaso(self.semente);
        for mao in [Mao::Esquerda, Mao::Direita] {
            let da_mao: Vec<(&i64, &Vec<usize>)> =
                momentos.iter().filter(|((m, _), _)| *m == mao).map(|((_, t), v)| (t, v)).collect();
            let teclas: Vec<Vec<(u8, Option<u8>)>> =
                da_mao.iter().map(|(_, idx)| idx.iter().map(|&i| (p.notas[i].tecla, p.notas[i].dedo)).collect()).collect();
            let posicoes = dedilhado::dedilhar_com_sugestoes(mao, &teclas);

            for ((_, idx), pos) in da_mao.iter().zip(&posicoes) {
                let tempo = p.notas[idx[0]].inicio;
                if !pos.cabe {
                    let nomes: Vec<String> = pos.teclas.iter().map(|&t| corda::nome(t as usize)).collect();
                    avisos.push(format!(
                        "{}: o acorde {} não cabe na {}; arpejado",
                        texto_tempo(tempo),
                        nomes.join("-"),
                        if mao == Mao::Direita { "mão direita" } else { "mão esquerda" }
                    ));
                }
                // 4. tempo: o acorde inteiro varia junto, cada nota um pouco
                let base = relogio.segundos(tempo) + 0.010 * h * acaso.normal();
                let aguda = *pos.teclas.last().unwrap();
                let acento_metrico = acento_metrico(p, tempo) * h as f32;
                for (k, (&tecla, &dedo)) in pos.teclas.iter().zip(&pos.dedos).enumerate() {
                    // a mesma tecla escrita em duas vozes é tocada uma vez,
                    // segurada pela maior das durações
                    let indice = *idx
                        .iter()
                        .filter(|&&i| p.notas[i].tecla == tecla)
                        .max_by(|&&a, &&b| p.notas[a].duracao.total_cmp(&p.notas[b].duracao))
                        .unwrap();
                    let nota = &p.notas[indice];
                    let arpejo = if pos.cabe { 0.0 } else { ARPEJO_S * k as f64 };
                    // a melodia (a nota mais aguda da mão direita num acorde)
                    // sai um pouco antes e mais forte
                    let e_melodia = mao == Mao::Direita && tecla == aguda && pos.teclas.len() > 1;
                    let adianta = if e_melodia { 0.012 * h } else { 0.0 };
                    let inicio_s = (base + arpejo - adianta + 0.003 * h * acaso.normal()).max(0.0);
                    let escrito = relogio.segundos(nota.inicio + nota.duracao) - relogio.segundos(nota.inicio);
                    let melodia = if e_melodia { 0.06 } else { 0.0 };
                    // 6. articulações
                    let art = nota.articulacao;
                    let ligado = if art.staccatissimo {
                        0.25
                    } else if art.staccato {
                        0.45
                    } else if art.tenuto {
                        1.0
                    } else {
                        self.legato
                    };
                    let segura = if art.fermata { 1.6 } else { 1.0 };
                    let acento = if art.acento { 0.15 } else { 0.0 };
                    let vel = (nota.dinamica + melodia + acento + acento_metrico + 0.04 * h as f32 * acaso.normal() as f32).clamp(0.05, 1.0);
                    tocadas.push(NotaTocada {
                        inicio_s,
                        fim_s: inicio_s + escrito * ligado * segura,
                        tecla,
                        mao,
                        dedo,
                        vel,
                        tempo,
                        indice,
                    });
                }
            }
        }
        tocadas.sort_by(|a, b| a.inicio_s.total_cmp(&b.inicio_s));

        // 3. corpo: o mesmo dedo (mesma mão) ou a mesma tecla só voltam
        // depois de soltos
        for i in 0..tocadas.len() {
            let (antes, depois) = tocadas.split_at_mut(i + 1);
            let atual = &mut antes[i];
            let proximo = depois.iter().find(|n| {
                (n.mao == atual.mao && n.dedo == atual.dedo) || n.tecla == atual.tecla
            });
            if let Some(n) = proximo {
                let limite = n.inicio_s - FOLGA_S;
                if atual.fim_s > limite {
                    atual.fim_s = limite.max(atual.inicio_s + 0.01);
                }
            }
        }

        // avisos em ordem de tempo ("tempo 1,00" antes de "tempo 3,00")
        avisos.sort_by(|a, b| {
            let tempo = |s: &str| s.split(':').next().and_then(|t| t.trim_start_matches("tempo ").replace(',', ".").parse::<f64>().ok());
            tempo(a).unwrap_or(0.0).total_cmp(&tempo(b).unwrap_or(0.0))
        });
        // 5. pé
        let pedais = self.pisadas(p, &relogio);
        let roteiro = montar_roteiro(&pedais, &tocadas, taxa);
        Execucao { roteiro, notas: tocadas, avisos, pedais, relogio }
    }
}

/// Ritardando nos dois últimos compassos: no fim, cada semínima dura até
/// 35% mais (com humanização 1).
fn relogio_da_execucao(p: &Partitura, h: f64) -> RelogioExecucao {
    let andamentos = Partitura { andamentos: p.andamentos.clone(), ..Partitura::default() };
    let fim = p.fim();
    let inicios = p.inicios_de_compasso(fim);
    let ini = if inicios.len() >= 3 { inicios[inicios.len() - 2].min(fim) } else { fim * 0.75 };
    let ini = if fim - ini < 1.0 { (fim - 4.0).max(0.0) } else { ini };
    RelogioExecucao { andamentos, ritardando: (ini, (fim - ini).max(0.0), 0.35 * h) }
}

/// Acento métrico (com humanização 1): primeiro tempo do compasso mais
/// forte, terceiro de um compasso de 4 um pouco, contratempos mais leves.
fn acento_metrico(p: &Partitura, tempo: f64) -> f32 {
    let (_, no_compasso) = p.compasso_e_tempo(tempo);
    let (tempos, _) = p.formula_em(tempo);
    let batida = no_compasso.round();
    if (no_compasso - batida).abs() > 1e-6 {
        -0.02
    } else if batida == 1.0 {
        0.05
    } else if tempos == 4 && batida == 3.0 {
        0.025
    } else {
        0.0
    }
}

/// Cabe num acorde? (as classes de nota, como bits 0..11, formam um
/// subconjunto de uma tríade ou de um acorde de sétima em alguma fundamental)
fn cabe_num_acorde(classes: u16) -> bool {
    const ACORDES: [&[u16]; 10] = [
        &[0, 4, 7],      // maior
        &[0, 3, 7],      // menor
        &[0, 3, 6],      // diminuta
        &[0, 4, 8],      // aumentada
        &[0, 5, 7],      // suspensa
        &[0, 4, 7, 10],  // dominante com sétima
        &[0, 4, 7, 11],  // maior com sétima maior
        &[0, 3, 7, 10],  // menor com sétima
        &[0, 3, 6, 10],  // meio-diminuto
        &[0, 3, 6, 9],   // diminuto com sétima
    ];
    (0..12u16).any(|raiz| {
        ACORDES.iter().any(|a| {
            let molde = a.iter().fold(0u16, |m, i| m | 1 << ((raiz + i) % 12));
            classes & !molde == 0
        })
    })
}

impl Player {
    /// O que o pé faz com cada pedal, com os instantes reais.
    fn pisadas(&self, p: &Partitura, relogio: &RelogioExecucao) -> Vec<Pisada> {
        let escolher = |marcas: &[(f64, f64)], automatico: fn(&Partitura) -> Vec<(f64, f64)>| -> (Vec<(f64, f64)>, bool) {
            match self.pe {
                Pe::Auto if marcas.is_empty() => (automatico(p), true),
                Pe::Auto | Pe::Partitura => (marcas.to_vec(), false),
                Pe::Automatico => (automatico(p), true),
                Pe::Nenhum => (Vec::new(), false),
            }
        };
        let pedais = [
            // sustain e sostenuto sincopados (depois das notas); una corda antes
            (Pedal::Sustain, escolher(&p.pedais, pedal_automatico), ATRASO_PEDAL_S, 0.0),
            (Pedal::Sostenuto, escolher(&p.sostenuto, |_| Vec::new()), ATRASO_PEDAL_S, 0.0),
            (Pedal::UnaCorda, escolher(&p.una_corda, una_corda_automatica), -ANTECIPA_UNA_CORDA_S, -ANTECIPA_UNA_CORDA_S),
        ];
        let mut v: Vec<Pisada> = pedais
            .into_iter()
            .flat_map(|(pedal, (trechos, automatica), desce, sobe)| {
                trechos.into_iter().map(move |(abaixa, solta)| (pedal, abaixa, solta, desce, sobe, automatica))
            })
            .map(|(pedal, abaixa, solta, desce, sobe, automatica)| Pisada {
                pedal,
                abaixa,
                solta,
                abaixa_s: (relogio.segundos(abaixa) + desce).max(0.0),
                solta_s: relogio.segundos(solta) + sobe,
                valor: 1.0,
                automatica,
            })
            .collect();
        v.sort_by(|a, b| a.abaixa.total_cmp(&b.abaixa));
        v
    }
}

/// O pé decide o una corda sozinho: abaixado nos trechos em pp ou mais
/// suave (de pelo menos uma semínima), solto quando entra uma nota mais
/// forte.
pub fn una_corda_automatica(p: &Partitura) -> Vec<(f64, f64)> {
    let mut notas: Vec<&crate::partitura::Nota> = p.notas.iter().collect();
    notas.sort_by(|a, b| a.inicio.total_cmp(&b.inicio));
    let mut trechos = Vec::new();
    // trecho em curso: (início, fim da última nota suave)
    let mut atual: Option<(f64, f64)> = None;
    for n in notas {
        if n.dinamica <= DINAMICA_UNA_CORDA + 1e-3 {
            let fim = n.inicio + n.duracao;
            atual = Some(atual.map_or((n.inicio, fim), |(ini, f)| (ini, f.max(fim))));
        } else if let Some((ini, f)) = atual.take() {
            trechos.push((ini, f.min(n.inicio)));
        }
    }
    trechos.extend(atual);
    trechos.retain(|(a, b)| b - a >= 1.0);
    trechos
}

/// O pé decide o sustain sozinho, pela harmonia da mão esquerda: mantém o
/// pedal enquanto as notas acumuladas no trecho ainda cabem num acorde, e
/// troca quando deixam de caber ou quando o baixo muda num tempo forte (o 1º
/// do compasso, ou o 3º num compasso de 4); solta nos silêncios. Assim um
/// baixo de Alberti (Dó-Sol-Mi-Sol) fica num pedal só, e Dó maior seguido de
/// Lá menor troca o pedal.
pub fn pedal_automatico(p: &Partitura) -> Vec<(f64, f64)> {
    let fim = p.fim();
    if fim <= 0.0 {
        return Vec::new();
    }
    // a harmonia vem da pauta de baixo; peças de uma pauta só usam tudo
    let tem_baixo = p.notas.iter().any(|n| n.pauta == Pauta::Inferior);
    let harmonia = |n: &&crate::partitura::Nota| !tem_baixo || n.pauta == Pauta::Inferior;

    let inicios = p.inicios_de_compasso(fim);
    // batidas: (instante, é tempo forte)
    let mut batidas = Vec::new();
    for (i, &ini) in inicios.iter().enumerate() {
        let proximo = inicios.get(i + 1).copied().unwrap_or(fim);
        let (tempos, unidade) = p.formula_em(ini);
        let passo = 4.0 / unidade as f64;
        let mut t = ini;
        let mut k = 0;
        while t < proximo - 1e-9 && t < fim {
            batidas.push((t, k == 0 || (tempos == 4 && k == 2)));
            t += passo;
            k += 1;
        }
    }
    let mut trechos = Vec::new();
    // trecho de pedal em curso: (início, classes de nota acumuladas, baixo)
    let mut atual: Option<(f64, u16, u8)> = None;
    for (j, &(b, forte)) in batidas.iter().enumerate() {
        let b_fim = batidas.get(j + 1).map_or(fim, |x| x.0);
        let soando = p.notas.iter().any(|n| n.inicio < b_fim - 1e-9 && n.inicio + n.duracao > b + 1e-9);
        if !soando {
            // silêncio: o pé solta
            if let Some((ini, _, _)) = atual.take() {
                trechos.push((ini, b));
            }
            continue;
        }
        let novas: Vec<u8> = p.notas.iter().filter(harmonia).filter(|n| n.inicio >= b - 1e-9 && n.inicio < b_fim - 1e-9).map(|n| n.tecla).collect();
        let Some(&grave) = novas.iter().min() else { continue };
        let classes = novas.iter().fold(0u16, |m, t| m | 1 << (t % 12));
        let baixo = grave % 12;
        atual = match atual {
            None => Some((b, classes, baixo)),
            Some((ini, acumuladas, baixo_atual)) => {
                let juntas = acumuladas | classes;
                if !cabe_num_acorde(juntas) || (forte && baixo != baixo_atual) {
                    trechos.push((ini, b));
                    Some((b, classes, baixo))
                } else {
                    Some((ini, juntas, baixo_atual))
                }
            }
        };
    }
    if let Some((ini, _, _)) = atual {
        trechos.push((ini, fim));
    }
    trechos
}

/// Mais de 5 notas numa mão: as que sobram (as mais graves da direita, as
/// mais agudas da esquerda) passam para a outra mão, se ela tiver dedos
/// livres naquele instante; senão são descartadas.
fn redistribuir(p: &Partitura, momentos: &mut BTreeMap<(Mao, i64), Vec<usize>>, avisos: &mut Vec<String>) {
    let cheios: Vec<(Mao, i64)> = momentos.iter().filter(|(_, v)| v.len() > DEDOS).map(|(k, _)| *k).collect();
    for (mao, t) in cheios {
        let outra = if mao == Mao::Direita { Mao::Esquerda } else { Mao::Direita };
        let mut notas = momentos.remove(&(mao, t)).unwrap();
        notas.sort_by_key(|&i| p.notas[i].tecla);
        if mao == Mao::Esquerda {
            notas.reverse(); // as que sobram ficam no começo
        }
        let sobra: Vec<usize> = notas.drain(..notas.len() - DEDOS).collect();
        momentos.insert((mao, t), notas);
        let livres = DEDOS - momentos.get(&(outra, t)).map_or(0, |v| v.len());
        for (k, &i) in sobra.iter().enumerate() {
            let nome = corda::nome(p.notas[i].tecla as usize);
            let tempo = texto_tempo(p.notas[i].inicio);
            if k < livres {
                momentos.entry((outra, t)).or_default().push(i);
                avisos.push(format!("{tempo}: mais de 5 notas numa mão; {nome} passou para a outra mão"));
            } else {
                avisos.push(format!("{tempo}: mais de 10 notas ao mesmo tempo; {nome} não foi tocada"));
            }
        }
    }
}

fn montar_roteiro(pedais: &[Pisada], tocadas: &[NotaTocada], taxa: u32) -> Roteiro {
    let mut r = Roteiro::new(taxa);
    let mut fim = 0f64;
    for n in tocadas {
        r.evento(n.inicio_s, Evento::NotaOn { tecla: n.tecla, vel: n.vel });
        r.evento(n.fim_s, Evento::NotaOff { tecla: n.tecla });
        fim = fim.max(n.fim_s);
    }
    // 5. pé
    for d in pedais {
        r.pedal(d.abaixa_s, d.pedal, d.valor);
        r.pedal(d.solta_s, d.pedal, 0.0);
        fim = fim.max(d.solta_s);
    }
    // o que mostrar durante a execução: cada momento com mão e dedos
    let mut por_momento: BTreeMap<(i64, Mao), Vec<&NotaTocada>> = BTreeMap::new();
    for n in tocadas {
        por_momento.entry((instante(n.tempo), n.mao)).or_default().push(n);
    }
    for ((_, mao), mut notas) in por_momento {
        notas.sort_by_key(|n| n.tecla);
        let texto: Vec<String> = notas.iter().map(|n| format!("{}({})", corda::nome(n.tecla as usize), n.dedo)).collect();
        let t = notas.iter().map(|n| n.inicio_s).fold(f64::INFINITY, f64::min);
        r.marco(t, &format!("{} {}", mao.sigla(), texto.join(" ")));
    }
    r.marcos.sort_by_key(|m| m.0);
    r.duracao = r.quadro(fim);
    r.cauda(2.5);
    r
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::partitura::exemplo;

    const TAXA: u32 = 48000;

    fn exato() -> Player {
        Player { humanizacao: 0.0, ..Player::default() }
    }

    fn dedos(e: &Execucao, mao: Mao) -> Vec<u8> {
        e.notas.iter().filter(|n| n.mao == mao).map(|n| n.dedo).collect()
    }

    #[test]
    fn escala_de_duas_oitavas_com_dedilhado_padrao() {
        let e = exato().tocar(&exemplo("escala").unwrap(), TAXA);
        // a direita sai exatamente como se ensina, subindo e descendo
        let md = dedos(&e, Mao::Direita);
        assert_eq!(md[..15], [1, 2, 3, 1, 2, 3, 4, 1, 2, 3, 1, 2, 3, 4, 5], "subida da direita");
        assert_eq!(md[14..], [5, 4, 3, 2, 1, 3, 2, 1, 4, 3, 2, 1, 3, 2, 1], "descida da direita");
        // a esquerda sai com um dedilhado alternativo tocável (o modelo não
        // reproduz o 5-4-3-2-1-3-2-1-4-... pedagógico, ver README): confere
        // que nunca repete o dedo em teclas seguidas
        let me: Vec<&NotaTocada> = e.notas.iter().filter(|n| n.mao == Mao::Esquerda).collect();
        for par in me.windows(2) {
            assert!(par[0].dedo != par[1].dedo || par[0].tecla == par[1].tecla, "{:?}", par);
        }
        assert!(e.avisos.is_empty(), "{:?}", e.avisos);
    }

    #[test]
    fn sem_humanizacao_os_tempos_sao_exatos() {
        let p = exemplo("escala").unwrap();
        let e = exato().tocar(&p, TAXA);
        for n in &e.notas {
            let esperado = INICIO_S + p.segundos(n.tempo);
            assert!((n.inicio_s - esperado).abs() < 1e-9, "{n:?}");
        }
    }

    #[test]
    fn humanizacao_varia_mas_e_repetivel() {
        let p = exemplo("acordes").unwrap();
        let humano = Player { humanizacao: 1.0, ..Player::default() };
        let (a, b) = (humano.tocar(&p, TAXA), humano.tocar(&p, TAXA));
        assert_eq!(a.notas, b.notas, "mesma semente, mesma execução");
        let e = exato().tocar(&p, TAXA);
        let diferenca = a.notas.iter().zip(&e.notas).map(|(x, y)| (x.inicio_s - y.inicio_s).abs()).fold(0.0, f64::max);
        assert!(diferenca > 0.001 && diferenca < 0.05, "variação de {diferenca} s");
    }

    #[test]
    fn corpo_dedo_e_tecla_so_voltam_depois_de_soltos() {
        for nome in crate::partitura::EXEMPLOS {
            let e = Player::default().tocar(&exemplo(nome).unwrap(), TAXA);
            for (i, a) in e.notas.iter().enumerate() {
                for b in &e.notas[i + 1..] {
                    let mesmo_dedo = a.mao == b.mao && a.dedo == b.dedo;
                    if (mesmo_dedo || a.tecla == b.tecla) && b.inicio_s < a.fim_s {
                        panic!("{nome}: {a:?} ainda abaixada quando {b:?} começa");
                    }
                }
            }
        }
    }

    #[test]
    fn no_maximo_cinco_notas_por_mao() {
        let e = exato().tocar(&exemplo("desafios").unwrap(), TAXA);
        let mut por_instante: BTreeMap<(i64, Mao), usize> = BTreeMap::new();
        for n in &e.notas {
            *por_instante.entry((instante(n.tempo), n.mao)).or_default() += 1;
        }
        assert!(por_instante.values().all(|&n| n <= DEDOS), "{por_instante:?}");
        assert!(e.avisos.iter().any(|a| a.contains("passou para a outra mão")), "{:?}", e.avisos);
    }

    #[test]
    fn acorde_largo_e_arpejado() {
        let e = exato().tocar(&exemplo("desafios").unwrap(), TAXA);
        assert!(e.avisos.iter().any(|a| a.contains("C4-E5") && a.contains("arpejado")), "{:?}", e.avisos);
        let c4 = e.notas.iter().find(|n| n.tecla == corda::tecla_de_nome("C4").unwrap() as u8 && n.tempo == 0.0).unwrap();
        let e5 = e.notas.iter().find(|n| n.tecla == corda::tecla_de_nome("E5").unwrap() as u8).unwrap();
        assert!((e5.inicio_s - c4.inicio_s - ARPEJO_S).abs() < 1e-9);
    }

    #[test]
    fn mesma_tecla_em_duas_vozes_e_tocada_uma_vez() {
        use crate::partitura::{Nota, Pauta};
        let n = |tecla, duracao, voz| Nota { duracao, tecla, pauta: Pauta::Superior, dinamica: 0.5, voz, ..Nota::default() };
        let p = Partitura { notas: vec![n(40, 1.0, 1), n(40, 3.0, 2), n(44, 1.0, 1)], ..Partitura::default() };
        let e = exato().tocar(&p, TAXA);
        assert_eq!(e.notas.len(), 2);
        let mi = e.notas.iter().find(|t| t.tecla == 40).unwrap();
        assert_eq!(mi.indice, 1, "segura pela maior duração");
    }

    use crate::partitura::{Articulacao, Nota};

    fn nota_em(inicio: f64, duracao: f64, nome: &str, pauta: Pauta) -> Nota {
        let tecla = corda::tecla_de_nome(nome).unwrap() as u8;
        Nota { inicio, duracao, tecla, pauta, dinamica: 0.5, voz: if pauta == Pauta::Superior { 1 } else { 5 }, ..Nota::default() }
    }

    #[test]
    fn pe_troca_o_pedal_quando_a_harmonia_muda() {
        // 2 compassos de Dó em baixo de Alberti (C-G-E-G em colcheias), 1 de
        // Sol (G-D-B-D), uma pausa de compasso inteiro e um acorde final
        let mut notas = Vec::new();
        let alberti = |base: [&str; 4], ini: f64, notas: &mut Vec<Nota>| {
            for i in 0..8 {
                notas.push(nota_em(ini + i as f64 * 0.5, 0.5, base[i % 4], Pauta::Inferior));
            }
        };
        alberti(["C3", "G3", "E3", "G3"], 0.0, &mut notas);
        alberti(["C3", "G3", "E3", "G3"], 4.0, &mut notas);
        alberti(["G2", "D3", "B2", "D3"], 8.0, &mut notas);
        notas.push(nota_em(16.0, 4.0, "C3", Pauta::Inferior));
        let p = Partitura { notas, ..Partitura::default() };
        let pedais = pedal_automatico(&p);
        // Dó (0 a 8), Sol (8 a 12), solta na pausa, Dó final (16 a 20)
        assert_eq!(pedais, [(0.0, 8.0), (8.0, 12.0), (16.0, 20.0)], "{pedais:?}");
        // Dó maior e depois Lá menor (cabem juntos em Lá menor com sétima, mas o
        // baixo muda no tempo forte): troca
        let p2 = Partitura {
            notas: vec![nota_em(0.0, 4.0, "C3", Pauta::Inferior), nota_em(0.0, 4.0, "E3", Pauta::Inferior), nota_em(0.0, 4.0, "G3", Pauta::Inferior),
                        nota_em(4.0, 4.0, "A2", Pauta::Inferior), nota_em(4.0, 4.0, "E3", Pauta::Inferior)],
            ..Partitura::default()
        };
        assert_eq!(pedal_automatico(&p2), [(0.0, 4.0), (4.0, 8.0)]);
        // sem marcações na partitura, o pé automático entra sozinho
        let e = exato().tocar(&p, TAXA);
        assert_eq!(trechos(&e, Pedal::Sustain), pedais);
        assert!(e.pedais.iter().all(|d| d.automatica));
        let so_partitura = Player { pe: Pe::Partitura, ..exato() }.tocar(&p, TAXA);
        assert!(so_partitura.pedais.is_empty());
        let nenhum = Player { pe: Pe::Nenhum, ..exato() }.tocar(&exemplo("acordes").unwrap(), TAXA);
        assert!(nenhum.roteiro.eventos.iter().all(|x| !matches!(x.evento, Evento::Pedal { .. })));
    }

    fn trechos(e: &Execucao, pedal: Pedal) -> Vec<(f64, f64)> {
        e.pedais.iter().filter(|d| d.pedal == pedal).map(|d| (d.abaixa, d.solta)).collect()
    }

    #[test]
    fn una_corda_no_pp_e_sostenuto_da_partitura() {
        // 4 semínimas em pp, 2 em f, 1 em pp (curto demais para o pé)
        let mut notas: Vec<Nota> = (0..7).map(|i| nota_em(i as f64, 1.0, "C4", Pauta::Superior)).collect();
        for (i, n) in notas.iter_mut().enumerate() {
            n.dinamica = if (4..6).contains(&i) { 0.7 } else { 0.25 };
        }
        notas[6].duracao = 0.5;
        let p = Partitura { notas, sostenuto: vec![(1.0, 3.0)], ..Partitura::default() };
        assert_eq!(una_corda_automatica(&p), [(0.0, 4.0)]);
        let e = exato().tocar(&p, TAXA);
        assert_eq!(trechos(&e, Pedal::UnaCorda), [(0.0, 4.0)]);
        assert_eq!(trechos(&e, Pedal::Sostenuto), [(1.0, 3.0)]);
        // una corda desce antes da 1ª nota; sostenuto depois da nota em que pega
        let uc = e.pedais.iter().find(|d| d.pedal == Pedal::UnaCorda).unwrap();
        let so = e.pedais.iter().find(|d| d.pedal == Pedal::Sostenuto).unwrap();
        let inicio = |i: usize| e.notas.iter().find(|n| n.indice == i).unwrap().inicio_s;
        assert!(uc.abaixa_s < inicio(0) || uc.abaixa_s == 0.0);
        assert!(uc.solta_s < inicio(4));
        assert!(so.abaixa_s > inicio(1) && !so.automatica);
        // cada pisada vira um evento de abaixar e um de soltar no roteiro
        let ev = |pedal| e.roteiro.eventos.iter().filter(|x| matches!(x.evento, Evento::Pedal { pedal: q, .. } if q == pedal)).count();
        assert_eq!(ev(Pedal::UnaCorda), 2);
        assert_eq!(ev(Pedal::Sostenuto), 2);
        assert!(Player { pe: Pe::Nenhum, ..exato() }.tocar(&p, TAXA).pedais.is_empty());
    }

    #[test]
    fn acordes_reconhecidos() {
        let c = |notas: &[u16]| notas.iter().fold(0u16, |m, n| m | 1 << (n % 12));
        assert!(cabe_num_acorde(c(&[0, 4, 7]))); // Dó maior
        assert!(cabe_num_acorde(c(&[9, 0, 4]))); // Lá menor
        assert!(cabe_num_acorde(c(&[7, 11, 2, 5]))); // Sol com sétima
        assert!(cabe_num_acorde(c(&[0, 7]))); // quinta solta
        assert!(!cabe_num_acorde(c(&[0, 2, 4]))); // Dó-Ré-Mi: não é acorde
        assert!(!cabe_num_acorde(c(&[0, 4, 7, 2, 11]))); // Dó + Sol maior
    }

    #[test]
    fn articulacoes_mudam_duracao_e_forca() {
        let mut notas: Vec<Nota> = ["C4", "D4", "E4", "F4"].iter().enumerate().map(|(i, n)| nota_em(i as f64, 1.0, n, Pauta::Superior)).collect();
        notas[1].articulacao = Articulacao { staccato: true, ..Articulacao::default() };
        notas[2].articulacao = Articulacao { acento: true, ..Articulacao::default() };
        notas[3].articulacao = Articulacao { fermata: true, ..Articulacao::default() };
        let p = Partitura { notas, ..Partitura::default() };
        let e = Player { pe: Pe::Nenhum, ..exato() }.tocar(&p, TAXA);
        let dur = |i: usize| e.notas.iter().find(|n| n.indice == i).map(|n| n.fim_s - n.inicio_s).unwrap();
        let vel = |i: usize| e.notas.iter().find(|n| n.indice == i).unwrap().vel;
        assert!(dur(1) < 0.6 * dur(0), "staccato");
        assert!(vel(2) > vel(0) + 0.1, "acento");
        assert!(dur(3) > 1.4 * dur(0), "fermata");
    }

    #[test]
    fn ritardando_no_fim_e_relogio_nos_dois_sentidos() {
        let p = exemplo("escala").unwrap();
        let e = Player { humanizacao: 1.0, ..Player::default() }.tocar(&p, TAXA);
        let r = &e.relogio;
        let fim = p.fim();
        // no começo segue o andamento escrito; no fim, atrasa
        assert!((r.segundos(1.0) - (INICIO_S + p.segundos(1.0))).abs() < 1e-9);
        assert!(r.segundos(fim) > INICIO_S + p.segundos(fim) + 0.1);
        for t in [0.0, 3.0, fim - 2.0, fim] {
            assert!((r.tempo_em(r.segundos(t)) - t).abs() < 1e-6, "{t}");
        }
    }

    #[test]
    fn melodia_sai_antes_do_acompanhamento() {
        let p = exemplo("acordes").unwrap();
        let e = Player { humanizacao: 1.0, semente: 7, ..Player::default() }.tocar(&p, TAXA);
        // em cada acorde da direita, a mais aguda tende a sair antes das outras
        let mut adiantada = 0;
        let mut total = 0;
        for t in [0.0, 2.0, 4.0, 6.0, 8.0] {
            let mut d: Vec<&NotaTocada> = e.notas.iter().filter(|n| n.mao == Mao::Direita && n.tempo == t).collect();
            d.sort_by_key(|n| n.tecla);
            let aguda = d.last().unwrap().inicio_s;
            let resto = d[..d.len() - 1].iter().map(|n| n.inicio_s).sum::<f64>() / (d.len() - 1) as f64;
            adiantada += (aguda < resto) as usize;
            total += 1;
        }
        assert!(adiantada >= 4, "{adiantada} de {total}");
    }

    #[test]
    fn roteiro_tem_cada_nota_ligada_e_desligada_e_os_pedais() {
        let p = exemplo("acordes").unwrap();
        let e = Player::default().tocar(&p, TAXA);
        let conta = |f: &dyn Fn(&Evento) -> bool| e.roteiro.eventos.iter().filter(|x| f(&x.evento)).count();
        assert_eq!(conta(&|x| matches!(x, Evento::NotaOn { .. })), p.notas.len());
        assert_eq!(conta(&|x| matches!(x, Evento::NotaOff { .. })), p.notas.len());
        assert_eq!(conta(&|x| matches!(x, Evento::Pedal { .. })), 2 * p.pedais.len());
    }
}
