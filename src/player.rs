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
//! 5. pé (versão 0): executa as marcações de pedal da partitura, abaixando
//!    um pouco depois do tempo (pedal sincopado). Decidir sozinho quando
//!    pedalar fica para depois.

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
/// Tudo começa um pouco depois de zero, para a humanização poder adiantar.
const INICIO_S: f64 = 0.05;

pub struct Player {
    /// 0 = exato e repetível; 1 = variações de um pianista humano
    pub humanizacao: f32,
    pub semente: u32,
    /// fração da duração escrita em que a tecla fica abaixada
    pub legato: f64,
}

impl Default for Player {
    fn default() -> Self {
        Player { humanizacao: 0.5, semente: 1, legato: 0.95 }
    }
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
}

pub struct Execucao {
    pub roteiro: Roteiro,
    pub notas: Vec<NotaTocada>,
    pub avisos: Vec<String>,
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

        // 2. dedos, mão por mão
        let mut tocadas = Vec::new();
        let mut acaso = Acaso(self.semente);
        let h = self.humanizacao as f64;
        for mao in [Mao::Esquerda, Mao::Direita] {
            let da_mao: Vec<(&i64, &Vec<usize>)> =
                momentos.iter().filter(|((m, _), _)| *m == mao).map(|((_, t), v)| (t, v)).collect();
            let teclas: Vec<Vec<u8>> = da_mao.iter().map(|(_, idx)| idx.iter().map(|&i| p.notas[i].tecla).collect()).collect();
            let posicoes = dedilhado::dedilhar(mao, &teclas);

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
                let base = INICIO_S + p.segundos(tempo) + 0.010 * h * acaso.normal();
                let aguda = *pos.teclas.last().unwrap();
                for (k, (&tecla, &dedo)) in pos.teclas.iter().zip(&pos.dedos).enumerate() {
                    let nota = idx.iter().map(|&i| &p.notas[i]).find(|n| n.tecla == tecla).unwrap();
                    let arpejo = if pos.cabe { 0.0 } else { ARPEJO_S * k as f64 };
                    let inicio_s = (base + arpejo + 0.003 * h * acaso.normal()).max(0.0);
                    let escrito = p.segundos(nota.inicio + nota.duracao) - p.segundos(nota.inicio);
                    let melodia = if mao == Mao::Direita && tecla == aguda && pos.teclas.len() > 1 { 0.06 } else { 0.0 };
                    let vel = (nota.dinamica + melodia + 0.04 * h as f32 * acaso.normal() as f32).clamp(0.05, 1.0);
                    tocadas.push(NotaTocada {
                        inicio_s,
                        fim_s: inicio_s + escrito * self.legato,
                        tecla,
                        mao,
                        dedo,
                        vel,
                        tempo,
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
        let roteiro = montar_roteiro(p, &tocadas, taxa);
        Execucao { roteiro, notas: tocadas, avisos }
    }
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

fn montar_roteiro(p: &Partitura, tocadas: &[NotaTocada], taxa: u32) -> Roteiro {
    let mut r = Roteiro::new(taxa);
    let mut fim = 0f64;
    for n in tocadas {
        r.evento(n.inicio_s, Evento::NotaOn { tecla: n.tecla, vel: n.vel });
        r.evento(n.fim_s, Evento::NotaOff { tecla: n.tecla });
        fim = fim.max(n.fim_s);
    }
    // 5. pé: pedal sincopado, abaixa logo depois do golpe
    for &(abaixa, solta) in &p.pedais {
        r.pedal(INICIO_S + p.segundos(abaixa) + ATRASO_PEDAL_S, Pedal::Sustain, 1.0);
        let t = INICIO_S + p.segundos(solta);
        r.pedal(t, Pedal::Sustain, 0.0);
        fim = fim.max(t);
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
    fn roteiro_tem_cada_nota_ligada_e_desligada_e_os_pedais() {
        let p = exemplo("acordes").unwrap();
        let e = Player::default().tocar(&p, TAXA);
        let conta = |f: &dyn Fn(&Evento) -> bool| e.roteiro.eventos.iter().filter(|x| f(&x.evento)).count();
        assert_eq!(conta(&|x| matches!(x, Evento::NotaOn { .. })), p.notas.len());
        assert_eq!(conta(&|x| matches!(x, Evento::NotaOff { .. })), p.notas.len());
        assert_eq!(conta(&|x| matches!(x, Evento::Pedal { .. })), 2 * p.pedais.len());
    }
}
