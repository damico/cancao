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

//! Emoção percebida, estimada ao longo de uma execução, nas duas dimensões
//! do circumplexo de Russell: valência (negativa a positiva) e ativação
//! (calma a agitada), ambas de -1 a 1.
//!
//! É uma estimativa por regras, não uma medição nem um modelo treinado: os
//! pesos seguem a direção dos achados da literatura, para comparar
//! interpretações de uma mesma partitura.
//!
//! - Chowdhury e Widmer (2021, 2023): densidade de ataques ("velocidade
//!   percebida") e dinâmica tocada são os maiores preditores de ativação;
//!   articulação e ritmo também; a valência depende sobretudo do modo
//!   (maior ou menor). Janela de 5 s com passo de 1 s, traço suavizado.
//! - Battcock e Schutz (citados por eles): taxa de ataques pesa na
//!   ativação e um pouco na valência; registro mais agudo, mais positivo.
//! - Ching e Widmer (2026): a execução muda pouco a valência e mais a
//!   ativação; por isso as interpretações se separam mais na vertical.
//!
//! - Gabrielsson e Juslin (1996): tristeza e ternura se confundem nas
//!   avaliações; o que as separa na execução é a tristeza mais lenta e com
//!   desvios de tempo maiores, e a ternura com timbre suave.
//!
//! Por janela: ataques por segundo (um acorde conta uma vez), força média,
//! articulação (duração tocada sobre a escrita), agógica (variação do
//! andamento de tempo a tempo), una corda (timbre suave), modo (perfis de
//! Krumhansl e Kessler sobre as classes de nota) e registro médio.

use crate::eventos::Pedal;
use crate::partitura::Partitura;
use crate::player::Execucao;

/// Janela de análise e passo entre pontos (s).
const JANELA_S: f64 = 5.0;
const PASSO_S: f64 = 1.0;
/// O modo olha uma janela maior (a tonalidade muda devagar).
const JANELA_MODO_S: f64 = 8.0;
/// Notas que começam com menos que isto de diferença são um ataque só.
const MESMO_ATAQUE_S: f64 = 0.03;
/// Suavização entre pontos vizinhos (0 = nenhuma).
const SUAVIZA: f32 = 0.5;

/// Perfis de tonalidade de Krumhansl e Kessler (1982), a partir da tônica.
const MAIOR: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MENOR: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ponto {
    /// centro da janela, em segundos da execução
    pub s: f64,
    pub valencia: f32,
    pub ativacao: f32,
}

/// O que se mede numa janela.
#[derive(Debug, Clone, Copy, Default)]
struct Pistas {
    ataques_por_s: f64,
    forca: f32,
    articulacao: f64,
    modo: f64,
    altura: f64,
    /// desvio padrão relativo do andamento de tempo a tempo
    agogica: f64,
    /// fração da janela com o una corda abaixado
    una_corda: f64,
}

/// Correlação de Pearson.
fn correlacao(a: &[f64; 12], b: &[f64]) -> f64 {
    let ma = a.iter().sum::<f64>() / 12.0;
    let mb = b.iter().sum::<f64>() / 12.0;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for k in 0..12 {
        num += (a[k] - ma) * (b[k] - mb);
        da += (a[k] - ma).powi(2);
        db += (b[k] - mb).powi(2);
    }
    if da * db > 0.0 { num / (da * db).sqrt() } else { 0.0 }
}

/// Maior (+1) ou menor (-1): melhor tonalidade maior contra a melhor menor.
fn modo(classes: &[f64; 12]) -> f64 {
    if classes.iter().sum::<f64>() <= 0.0 {
        return 0.0;
    }
    let melhor = |perfil: &[f64; 12]| {
        (0..12)
            .map(|tonica| {
                let girado: Vec<f64> = (0..12).map(|k| classes[(k + tonica) % 12]).collect();
                correlacao(perfil, &girado)
            })
            .fold(f64::MIN, f64::max)
    };
    ((melhor(&MAIOR) - melhor(&MENOR)) * 6.0).tanh()
}

fn pistas(p: &Partitura, e: &Execucao, centro: f64) -> Option<Pistas> {
    let (ini, fim) = (centro - JANELA_S / 2.0, centro + JANELA_S / 2.0);
    let notas: Vec<_> = e.notas.iter().filter(|n| n.inicio_s >= ini && n.inicio_s < fim).collect();
    if notas.is_empty() {
        return None;
    }
    let mut inicios: Vec<f64> = notas.iter().map(|n| n.inicio_s).collect();
    inicios.sort_by(f64::total_cmp);
    let ataques = 1 + inicios.windows(2).filter(|w| w[1] - w[0] > MESMO_ATAQUE_S).count();
    let forca = notas.iter().map(|n| n.vel).sum::<f32>() / notas.len() as f32;
    let articulacao = notas
        .iter()
        .map(|n| {
            let escrita = &p.notas[n.indice];
            let s = e.relogio.segundos(escrita.inicio + escrita.duracao) - e.relogio.segundos(escrita.inicio);
            if s > 0.0 { ((n.fim_s - n.inicio_s) / s).min(1.2) } else { 1.0 }
        })
        .sum::<f64>()
        / notas.len() as f64;
    let altura = notas.iter().map(|n| n.tecla as f64).sum::<f64>() / notas.len() as f64;
    let mut classes = [0f64; 12];
    let (ini_m, fim_m) = (centro - JANELA_MODO_S / 2.0, centro + JANELA_MODO_S / 2.0);
    for n in e.notas.iter().filter(|n| n.inicio_s < fim_m && n.fim_s > ini_m) {
        // tecla 0 = A0: classe 0 = Dó
        classes[(n.tecla as usize + 9) % 12] += n.fim_s.min(fim_m) - n.inicio_s.max(ini_m);
    }
    let una_corda: f64 = e
        .pedais
        .iter()
        .filter(|d| d.pedal == Pedal::UnaCorda)
        .map(|d| (d.solta_s.min(fim) - d.abaixa_s.max(ini)).max(0.0))
        .sum::<f64>()
        / JANELA_S;
    Some(Pistas {
        ataques_por_s: ataques as f64 / JANELA_S,
        forca,
        articulacao,
        modo: modo(&classes),
        altura,
        agogica: agogica(p, e, e.relogio.tempo_em(ini.max(0.0)), e.relogio.tempo_em(fim)),
        una_corda,
    })
}

/// Variação do andamento tocado de tempo a tempo entre `t0` e `t1`
/// (semínimas): desvio padrão relativo à média.
fn agogica(p: &Partitura, e: &Execucao, t0: f64, t1: f64) -> f64 {
    let locais: Vec<f64> = (t0.floor().max(0.0) as usize..t1.ceil().max(0.0) as usize)
        .map(|b| {
            let b = b as f64;
            (p.segundos(b + 1.0) - p.segundos(b)) / (e.relogio.segundos(b + 1.0) - e.relogio.segundos(b))
        })
        .collect();
    let media = locais.iter().sum::<f64>() / locais.len().max(1) as f64;
    if locais.len() < 2 || media <= 0.0 {
        return 0.0;
    }
    (locais.iter().map(|x| (x - media).powi(2)).sum::<f64>() / locais.len() as f64).sqrt() / media
}

/// Valência e ativação a partir das pistas de uma janela.
fn estimar(x: &Pistas) -> (f32, f32) {
    // 3 ataques/s = neutro; 9 = +1; 1 = -1
    let densidade = ((x.ataques_por_s.max(0.1) / 3.0).ln() / 3f64.ln()).clamp(-1.5, 1.5);
    let forca = (x.forca as f64 - 0.5) / 0.25;
    // destacado (staccato) agita; ligado acalma
    let destacado = (0.85 - x.articulacao) / 0.3;
    let altura = (x.altura - 44.0) / 20.0;
    // 5% de variação de andamento = 1
    let agogica = (x.agogica / 0.05).min(3.0);
    let ativacao = (0.6 * densidade + 0.45 * forca + 0.2 * destacado).tanh();
    let valencia =
        (0.65 * x.modo + 0.15 * altura + 0.15 * densidade + 0.1 * destacado - 0.15 * agogica + 0.3 * x.una_corda).tanh();
    (valencia as f32, ativacao as f32)
}

/// Trajetória da emoção percebida, um ponto por segundo.
pub fn trajetoria(p: &Partitura, e: &Execucao) -> Vec<Ponto> {
    let fim = e.notas.iter().map(|n| n.fim_s).fold(0.0, f64::max);
    let mut v: Vec<Ponto> = Vec::new();
    let mut s = 0.0;
    while s <= fim + PASSO_S {
        let (valencia, ativacao) = match pistas(p, e, s) {
            Some(x) => estimar(&x),
            // silêncio: a ativação cai, a valência fica
            None => (v.last().map_or(0.0, |u| u.valencia), -0.8),
        };
        let ponto = match v.last() {
            Some(u) => Ponto {
                s,
                valencia: SUAVIZA * u.valencia + (1.0 - SUAVIZA) * valencia,
                ativacao: SUAVIZA * u.ativacao + (1.0 - SUAVIZA) * ativacao,
            },
            None => Ponto { s, valencia, ativacao },
        };
        v.push(ponto);
        s += PASSO_S;
    }
    v
}

/// Ponto da trajetória no instante `s` (interpolado).
pub fn em(trajetoria: &[Ponto], s: f64) -> Option<Ponto> {
    let i = trajetoria.partition_point(|u| u.s <= s);
    match (i.checked_sub(1).and_then(|j| trajetoria.get(j)), trajetoria.get(i)) {
        (Some(a), Some(b)) => {
            let u = ((s - a.s) / (b.s - a.s)) as f32;
            Some(Ponto { s, valencia: a.valencia + u * (b.valencia - a.valencia), ativacao: a.ativacao + u * (b.ativacao - a.ativacao) })
        }
        (Some(a), None) => Some(*a),
        (None, Some(b)) => Some(*b),
        (None, None) => None,
    }
}

/// O que distingue uma execução, medido nela mesma.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Resumo {
    pub duracao_s: f64,
    /// andamento médio em relação ao escrito (1 = como escrito)
    pub andamento: f64,
    /// força média e faixa (do 10º ao 90º percentil), 0..1
    pub forca: f32,
    pub faixa: f32,
    /// duração tocada sobre a escrita, média
    pub articulacao: f64,
    /// variação do andamento de tempo a tempo (desvio padrão relativo)
    pub agogica: f64,
    /// fração do tempo com o sustain e com o una corda abaixados
    pub sustain: f64,
    pub una_corda: f64,
    /// médias da trajetória
    pub valencia: f32,
    pub ativacao: f32,
}

pub fn resumir(p: &Partitura, e: &Execucao, trajetoria: &[Ponto]) -> Resumo {
    if e.notas.is_empty() {
        return Resumo::default();
    }
    let ini = e.notas.iter().map(|n| n.inicio_s).fold(f64::MAX, f64::min);
    let duracao_s = e.notas.iter().map(|n| n.fim_s).fold(0.0, f64::max) - ini;
    let fim = p.fim();
    let tocado = e.relogio.segundos(fim) - e.relogio.segundos(0.0);
    let andamento = if tocado > 0.0 { p.segundos(fim) / tocado } else { 1.0 };
    let mut forcas: Vec<f32> = e.notas.iter().map(|n| n.vel).collect();
    forcas.sort_by(f32::total_cmp);
    let percentil = |q: f64| forcas[((forcas.len() - 1) as f64 * q).round() as usize];
    let forca = forcas.iter().sum::<f32>() / forcas.len() as f32;
    let articulacao = e
        .notas
        .iter()
        .map(|n| {
            let escrita = &p.notas[n.indice];
            let s = e.relogio.segundos(escrita.inicio + escrita.duracao) - e.relogio.segundos(escrita.inicio);
            if s > 0.0 { ((n.fim_s - n.inicio_s) / s).min(1.2) } else { 1.0 }
        })
        .sum::<f64>()
        / e.notas.len() as f64;
    let agogica = agogica(p, e, 0.0, fim.floor());
    let coberto = |pedal| {
        // fold a partir de +0 (a soma vazia de f64 dá -0)
        let t = e.pedais.iter().filter(|d| d.pedal == pedal).fold(0.0, |t, d| t + d.solta_s - d.abaixa_s);
        if duracao_s > 0.0 { (t / duracao_s).min(1.0) } else { 0.0 }
    };
    let n = trajetoria.len().max(1) as f32;
    Resumo {
        duracao_s,
        andamento,
        forca,
        faixa: percentil(0.9) - percentil(0.1),
        articulacao,
        agogica,
        sustain: coberto(Pedal::Sustain),
        una_corda: coberto(Pedal::UnaCorda),
        valencia: trajetoria.iter().map(|u| u.valencia).sum::<f32>() / n,
        ativacao: trajetoria.iter().map(|u| u.ativacao).sum::<f32>() / n,
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::expressao::Perfil;
    use crate::partitura::exemplo;
    use crate::player::Player;

    fn tocar(nome: &str, perfil: Perfil) -> (Partitura, Execucao) {
        let p = exemplo(nome).unwrap();
        let e = Player { perfil, ..Player::default() }.tocar(&p, 48000);
        (p, e)
    }

    #[test]
    fn modo_maior_e_menor() {
        let classes = |notas: &[usize]| {
            let mut c = [0f64; 12];
            for &n in notas {
                c[n] += 1.0;
            }
            c
        };
        assert!(modo(&classes(&[0, 4, 7, 0, 4, 7, 2, 5, 9, 11])) > 0.3, "Dó maior");
        assert!(modo(&classes(&[9, 0, 4, 9, 0, 4, 2, 5, 8, 11])) < -0.3, "Lá menor");
    }

    #[test]
    fn energica_mais_ativada_que_melancolica() {
        for nome in ["escala", "acordes"] {
            let media = |perfil| {
                let (p, e) = tocar(nome, perfil);
                resumir(&p, &e, &trajetoria(&p, &e))
            };
            let (en, me, co) = (media(Perfil::Energica), media(Perfil::Melancolica), media(Perfil::Contemplativa));
            assert!(en.ativacao > me.ativacao + 0.2, "{nome}: {} {}", en.ativacao, me.ativacao);
            assert!(en.ativacao > co.ativacao + 0.2, "{nome}");
            // a execução mexe menos na valência que na ativação
            assert!((en.valencia - me.valencia).abs() < (en.ativacao - me.ativacao).abs(), "{nome}");
            assert!(en.andamento > 1.0 && me.andamento < 0.9);
            assert!(en.articulacao < me.articulacao);
            assert!(me.agogica > en.agogica);
            // tristeza e ternura: perto na ativação, a melancólica abaixo na valência
            assert!(me.valencia < co.valencia, "{nome}: {} {}", me.valencia, co.valencia);
        }
    }

    #[test]
    fn trajetoria_um_ponto_por_segundo_e_interpolada() {
        let (p, e) = tocar("acordes", Perfil::Neutra);
        let t = trajetoria(&p, &e);
        assert!(t.len() > 5);
        assert!(t.windows(2).all(|w| (w[1].s - w[0].s - 1.0).abs() < 1e-9));
        assert!(t.iter().all(|u| u.valencia.abs() <= 1.0 && u.ativacao.abs() <= 1.0));
        let meio = em(&t, 2.5).unwrap();
        assert!((meio.valencia - 0.5 * (t[2].valencia + t[3].valencia)).abs() < 1e-6);
        // a cadência termina em Dó maior: positiva no fim
        let ultimo_som = e.notas.iter().map(|n| n.inicio_s).fold(0.0, f64::max);
        assert!(em(&t, ultimo_som).unwrap().valencia > 0.0);
        // a escala de Dó maior soa positiva
        let (p, e) = tocar("escala", Perfil::Neutra);
        assert!(resumir(&p, &e, &trajetoria(&p, &e)).valencia > 0.0);
    }
}
