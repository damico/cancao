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

//! Motor de expressão (Expressive Performance Engine): transforma uma
//! partitura numa interpretação, decidindo tempo, dinâmica, articulação,
//! fraseado e pedal segundo um perfil expressivo.
//!
//! ```text
//! Partitura -> motor de expressão -> Player (mãos, dedos, corpo) -> instrumento
//! ```
//!
//! A saída, por nota escrita, é o "codec de execução" de Ching e Widmer
//! (2026, a partir do Basis Mixer): andamento local (o relógio), força,
//! desvio de tempo (melodia adiantada, ritmo pontuado mais ou menos
//! marcado) e articulação (fração da duração escrita que a tecla fica
//! abaixada). Mãos, dedos e o que o corpo permite ficam com o player.
//!
//! Os perfis são experimentais: partem das tendências descritas na
//! literatura, não são uma tradução determinística de emoções em
//! parâmetros.
//!
//! - Gabrielsson e Juslin (1996): perfis de pistas por emoção. Tristeza:
//!   andamento lento, desvios de tempo grandes, intensidade baixa,
//!   contrastes longo-curto suavizados, legato, ataques lentos. Ternura:
//!   parecido, mais suave. Alegria: rápido, intensidade moderada a forte,
//!   contrastes acentuados, articulação arejada, ataques rápidos, sem
//!   ritardando final marcado.
//! - Higuchi et al. (2011): a execução com atenção afetiva (tristeza) tem
//!   mais agógica, mais legato, menos clareza de pulso, intensidade média
//!   menor e mais variação de dinâmica (fraseado) que a execução atenta às
//!   notas.
//! - Repp (1992): ritardandos nas fronteiras de frase e seção, com forma
//!   parabólica dentro do gesto.
//! - Sloboda e Lehmann (2001): o que muda a emocionalidade percebida são
//!   desvios locais (nota final da frase ~20% mais longa, subito piano,
//!   melodia mais adiantada), concentrados no início e no fim das frases de
//!   quatro compassos; exageros soam grotescos.

use crate::partitura::{Partitura, Pauta};

/// Tudo começa um pouco depois de zero, para a humanização poder adiantar.
pub const INICIO_S: f64 = 0.05;
/// Compassos por frase, quando a partitura não diz (Sloboda e Lehmann:
/// frases de quatro compassos).
const COMPASSOS_POR_FRASE: usize = 4;
/// Resolução do relógio da execução (semínimas).
const PASSO: f64 = 1.0 / 16.0;

/// Perfil expressivo da interpretação.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Perfil {
    /// a interpretação de referência: o escrito com pouca expressão
    /// (acentos do compasso, melodia adiantada, ritardando final), na
    /// medida da humanização
    #[default]
    Neutra,
    /// ternura e serenidade: lenta, suave, ligada, una corda nos trechos
    /// suaves
    Contemplativa,
    /// alegria e energia: rápida, forte, articulação arejada, ritmo
    /// pontuado marcado, pulso claro, pedal curto
    Energica,
    /// tristeza: mais lenta, contida, legato, muita agógica, respiração no
    /// fim das frases
    Melancolica,
}

impl Perfil {
    pub const TODOS: [Perfil; 4] = [Perfil::Neutra, Perfil::Contemplativa, Perfil::Energica, Perfil::Melancolica];

    pub fn nome(self) -> &'static str {
        match self {
            Perfil::Neutra => "Neutra",
            Perfil::Contemplativa => "Contemplativa",
            Perfil::Energica => "Enérgica",
            Perfil::Melancolica => "Melancólica",
        }
    }

    /// Pelo nome sem acento (linha de comando).
    pub fn de_nome(nome: &str) -> Option<Perfil> {
        match nome {
            "neutra" => Some(Perfil::Neutra),
            "contemplativa" => Some(Perfil::Contemplativa),
            "energica" => Some(Perfil::Energica),
            "melancolica" => Some(Perfil::Melancolica),
            _ => None,
        }
    }
}

/// Os parâmetros que um perfil escolhe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parametros {
    /// multiplica o andamento escrito
    pub andamento: f64,
    /// soma à dinâmica escrita (0..1)
    pub dinamica: f32,
    /// fraseado: a força cresce até o meio da frase e volta
    pub arco_dinamica: f32,
    /// fração da duração escrita em que a tecla fica abaixada (notas sem
    /// articulação escrita)
    pub legato: f64,
    /// ritmo longo-curto (pontuado): > 0 marca mais (a curta atrasa e
    /// encurta), < 0 suaviza
    pub contraste: f64,
    /// agógica na frase: mais lenta nas pontas, mais rápida no meio
    /// (parábola de Repp); 0.1 = pontas cerca de 7% mais lentas
    pub rubato: f64,
    /// alongamento do último tempo de cada frase
    pub respiracao: f64,
    /// ritardando nos dois últimos compassos: no fim, cada tempo dura
    /// (1 + ritardando) vezes mais
    pub ritardando: f64,
    /// multiplica os acentos do compasso (clareza de pulso)
    pub acento_metrico: f32,
    /// quanto a melodia sai antes do acompanhamento (s)
    pub adianto_melodia: f64,
    /// profundidade do sustain (1 = no fundo)
    pub pedal: f32,
    /// o pé automático solta o sustain na metade do trecho (som mais claro)
    pub pedal_curto: bool,
    /// dinâmica até a qual o pé automático usa o una corda
    pub una_corda_ate: f32,
}

impl Parametros {
    /// Parâmetros de um perfil. `h` (humanização) escala a Neutra; os
    /// outros perfis são escolhas deliberadas e não dependem dela.
    pub fn de(perfil: Perfil, h: f64) -> Parametros {
        let neutra = Parametros {
            andamento: 1.0,
            dinamica: 0.0,
            arco_dinamica: 0.0,
            legato: 0.95,
            contraste: 0.0,
            rubato: 0.0,
            respiracao: 0.0,
            ritardando: 0.35 * h,
            acento_metrico: h as f32,
            adianto_melodia: 0.012 * h,
            pedal: 1.0,
            pedal_curto: false,
            una_corda_ate: 0.25,
        };
        match perfil {
            Perfil::Neutra => neutra,
            Perfil::Contemplativa => Parametros {
                andamento: 0.88,
                dinamica: -0.15,
                arco_dinamica: 0.08,
                legato: 0.98,
                contraste: -0.2,
                rubato: 0.06,
                respiracao: 0.12,
                ritardando: 0.45,
                acento_metrico: 0.5,
                adianto_melodia: 0.015,
                una_corda_ate: 0.40,
                ..neutra
            },
            Perfil::Energica => Parametros {
                andamento: 1.15,
                dinamica: 0.12,
                arco_dinamica: 0.06,
                legato: 0.70,
                contraste: 0.3,
                rubato: 0.02,
                respiracao: 0.03,
                ritardando: 0.08,
                acento_metrico: 1.6,
                adianto_melodia: 0.008,
                pedal: 0.6,
                pedal_curto: true,
                una_corda_ate: 0.0,
            },
            Perfil::Melancolica => Parametros {
                andamento: 0.80,
                dinamica: -0.12,
                arco_dinamica: 0.12,
                legato: 1.0,
                contraste: -0.3,
                rubato: 0.10,
                respiracao: 0.20,
                ritardando: 0.60,
                acento_metrico: 0.5,
                adianto_melodia: 0.025,
                una_corda_ate: 0.30,
                ..neutra
            },
        }
    }
}

/// Quando cada posição da partitura é tocada: o andamento escrito vezes o
/// fator de andamento da interpretação (perfil, frases, ritardando).
/// Converte nos dois sentidos (a interface usa para pôr o playhead onde o
/// som está).
#[derive(Debug, Clone, Default)]
pub struct RelogioExecucao {
    /// só os andamentos da partitura
    andamentos: Partitura,
    /// segundos a mais (ou a menos) que o escrito, acumulados até cada
    /// ponto da grade de `PASSO` semínimas
    extra: Vec<f64>,
    /// segundos a mais por semínima depois do fim da grade, já em
    /// proporção ao andamento: (fator - 1)
    fator_final: f64,
}

impl RelogioExecucao {
    /// `fator(t)`: quanto cada semínima em `t` dura a mais que o escrito
    /// (1 = como escrito).
    fn novo(p: &Partitura, fim: f64, fator: impl Fn(f64) -> f64) -> Self {
        let andamentos = Partitura { andamentos: p.andamentos.clone(), ..Partitura::default() };
        let n = (fim / PASSO).ceil().max(0.0) as usize;
        let mut extra = Vec::with_capacity(n + 1);
        extra.push(0.0);
        let mut acumulado = 0.0;
        for i in 0..n {
            let meio = (i as f64 + 0.5) * PASSO;
            let f = fator(meio);
            if f != 1.0 {
                acumulado += (f - 1.0) * 60.0 / andamentos.bpm_em(meio) * PASSO;
            }
            extra.push(acumulado);
        }
        RelogioExecucao { andamentos, extra, fator_final: fator(fim + PASSO) - 1.0 }
    }

    fn extra_em(&self, tempo: f64) -> f64 {
        if tempo <= 0.0 || self.extra.len() < 2 {
            return 0.0;
        }
        let x = tempo / PASSO;
        let i = x.floor() as usize;
        if i + 1 < self.extra.len() {
            let u = x - i as f64;
            self.extra[i] + u * (self.extra[i + 1] - self.extra[i])
        } else {
            let fim = (self.extra.len() - 1) as f64 * PASSO;
            self.extra[self.extra.len() - 1] + self.fator_final * 60.0 / self.andamentos.bpm_em(fim) * (tempo - fim)
        }
    }

    /// Instante (s) em que a posição `tempo` (semínimas) é tocada.
    pub fn segundos(&self, tempo: f64) -> f64 {
        INICIO_S + self.andamentos.segundos(tempo) + self.extra_em(tempo)
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

/// Como tocar uma nota escrita.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NotaExpressiva {
    /// força, 0..1 (antes da variação aleatória da humanização)
    pub vel: f32,
    /// quanto sai antes do tempo (s); negativo = atrasa
    pub adianto_s: f64,
    /// fração da duração escrita (já com articulação e fermata)
    pub ligado: f64,
}

/// A interpretação de uma partitura.
#[derive(Debug, Clone)]
pub struct Interpretacao {
    pub perfil: Perfil,
    pub parametros: Parametros,
    pub relogio: RelogioExecucao,
    /// frases (início, fim), em semínimas
    pub frases: Vec<(f64, f64)>,
    /// uma por nota escrita, na ordem de `Partitura::notas`
    pub notas: Vec<NotaExpressiva>,
}

/// Frases: grupos de quatro compassos, cortados também depois de cada
/// fermata. Uma sobra de menos de um tempo no fim junta com a frase
/// anterior.
pub fn frases(p: &Partitura) -> Vec<(f64, f64)> {
    let fim = p.fim();
    if fim <= 0.0 {
        return Vec::new();
    }
    let inicios = p.inicios_de_compasso(fim);
    let mut cortes: Vec<f64> = inicios.iter().step_by(COMPASSOS_POR_FRASE).copied().collect();
    for n in p.notas.iter().filter(|n| n.articulacao.fermata) {
        cortes.push(n.inicio + n.duracao);
    }
    cortes.push(0.0);
    cortes.sort_by(f64::total_cmp);
    cortes.dedup_by(|a, b| (*a - *b).abs() < 1.0);
    cortes.retain(|&c| c < fim - 1.0);
    let mut v: Vec<(f64, f64)> = cortes.windows(2).map(|w| (w[0], w[1])).collect();
    v.push((*cortes.last().unwrap_or(&0.0), fim));
    v
}

/// Posição (0..1) de `tempo` dentro da sua frase, e o fim dela.
fn na_frase(frases: &[(f64, f64)], tempo: f64) -> Option<(f64, f64)> {
    let i = frases.partition_point(|f| f.1 <= tempo);
    frases.get(i).filter(|f| f.0 <= tempo).map(|&(a, b)| ((tempo - a) / (b - a), b))
}

/// Acento métrico (com peso 1): primeiro tempo do compasso mais forte,
/// terceiro de um compasso de 4 um pouco, contratempos mais leves.
pub fn acento_metrico(p: &Partitura, tempo: f64) -> f32 {
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

/// Interpreta a partitura com um perfil. `h`: humanização (0..1).
pub fn interpretar(p: &Partitura, perfil: Perfil, h: f64) -> Interpretacao {
    let par = Parametros::de(perfil, h);
    let frases = frases(p);
    let fim = p.fim();

    // ritardando final: nos dois últimos compassos
    let inicios = p.inicios_de_compasso(fim);
    let ini = if inicios.len() >= 3 { inicios[inicios.len() - 2].min(fim) } else { fim * 0.75 };
    let rit_ini = if fim - ini < 1.0 { (fim - 4.0).max(0.0) } else { ini };
    let rit_len = (fim - rit_ini).max(0.0);

    // tempo: andamento do perfil, arco da frase, respiração e ritardando
    let fator = |t: f64| {
        let mut f = 1.0 / par.andamento;
        if let Some((u, fim_frase)) = na_frase(&frases, t) {
            f *= 1.0 + par.rubato * (4.0 * (u - 0.5).powi(2) - 1.0 / 3.0);
            if fim_frase - t <= 1.0 {
                f *= 1.0 + par.respiracao;
            }
        }
        if rit_len > 0.0 && t > rit_ini {
            let u = ((t - rit_ini) / rit_len).min(1.0);
            f *= 1.0 + par.ritardando * u * u;
        }
        f
    };
    let relogio = RelogioExecucao::novo(p, fim, fator);

    // melodia: a mais aguda da pauta de cima onde ela tem acorde
    let mut melodia = vec![false; p.notas.len()];
    let mut i = 0;
    while i < p.notas.len() {
        let t = p.notas[i].inicio;
        let mesmo: Vec<usize> = (i..p.notas.len()).take_while(|&j| (p.notas[j].inicio - t).abs() < 1e-9).collect();
        let de_cima: Vec<usize> = mesmo.iter().copied().filter(|&j| p.notas[j].pauta == Pauta::Superior).collect();
        if de_cima.len() > 1 {
            let aguda = *de_cima.iter().max_by_key(|&&j| p.notas[j].tecla).unwrap();
            melodia[aguda] = true;
        }
        i += mesmo.len();
    }

    // ritmo pontuado: nota curta depois de uma longa na mesma pauta
    let mut atraso = vec![0f64; p.notas.len()];
    if par.contraste != 0.0 {
        for pauta in [Pauta::Superior, Pauta::Inferior] {
            let idx: Vec<usize> = (0..p.notas.len()).filter(|&j| p.notas[j].pauta == pauta).collect();
            for w in idx.windows(2) {
                let (a, b) = (&p.notas[w[0]], &p.notas[w[1]]);
                let seguida = (a.inicio + a.duracao - b.inicio).abs() < 1e-6;
                if seguida && a.duracao >= 1.5 * b.duracao && b.duracao <= 0.5 {
                    let d = par.contraste * 0.3 * b.duracao;
                    atraso[w[1]] = relogio.segundos(b.inicio + d) - relogio.segundos(b.inicio);
                }
            }
        }
    }

    let notas = p
        .notas
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let arco = na_frase(&frases, n.inicio).map_or(0.0, |(u, _)| par.arco_dinamica * (1.0 / 3.0 - 4.0 * (u as f32 - 0.5).powi(2)));
            let art = n.articulacao;
            let ligado = if art.staccatissimo {
                0.25
            } else if art.staccato {
                0.45
            } else if art.tenuto {
                1.0
            } else {
                par.legato
            };
            let segura = if art.fermata { 1.6 } else { 1.0 };
            let acento = if art.acento { 0.15 } else { 0.0 };
            let cantada = if melodia[i] { 0.06 } else { 0.0 };
            let vel = n.dinamica + par.dinamica + arco + cantada + acento + acento_metrico(p, n.inicio) * par.acento_metrico;
            let adianto_s = if melodia[i] { par.adianto_melodia } else { 0.0 } - atraso[i];
            NotaExpressiva { vel, adianto_s, ligado: ligado * segura }
        })
        .collect();

    Interpretacao { perfil, parametros: par, relogio, frases, notas }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::partitura::{exemplo, Nota};

    #[test]
    fn neutra_sem_humanizacao_segue_o_escrito() {
        let p = exemplo("escala").unwrap();
        let e = interpretar(&p, Perfil::Neutra, 0.0);
        for t in [0.0, 1.0, 7.5, p.fim(), p.fim() + 3.0] {
            assert!((e.relogio.segundos(t) - INICIO_S - p.segundos(t)).abs() < 1e-9, "{t}");
        }
        assert!(e.notas.iter().all(|n| n.adianto_s == 0.0 && n.ligado == 0.95));
    }

    #[test]
    fn frases_de_quatro_compassos() {
        let p = exemplo("escala").unwrap();
        let f = frases(&p);
        assert_eq!(f[0], (0.0, 16.0));
        assert_eq!(f.last().unwrap().1, p.fim());
        assert!(f.windows(2).all(|w| w[0].1 == w[1].0));
    }

    #[test]
    fn perfis_diferem_como_a_literatura_descreve() {
        let p = exemplo("acordes").unwrap();
        let duracao = |perfil| interpretar(&p, perfil, 0.5).relogio.segundos(p.fim());
        let forca = |perfil| {
            let e = interpretar(&p, perfil, 0.5);
            e.notas.iter().map(|n| n.vel).sum::<f32>() / e.notas.len() as f32
        };
        let ligado = |perfil| interpretar(&p, perfil, 0.5).notas[0].ligado;
        // melancólica e contemplativa mais lentas e suaves; enérgica mais
        // rápida, forte e destacada
        assert!(duracao(Perfil::Melancolica) > duracao(Perfil::Contemplativa));
        assert!(duracao(Perfil::Contemplativa) > duracao(Perfil::Neutra));
        assert!(duracao(Perfil::Energica) < duracao(Perfil::Neutra));
        assert!(forca(Perfil::Energica) > forca(Perfil::Neutra) + 0.1);
        assert!(forca(Perfil::Contemplativa) < forca(Perfil::Neutra) - 0.1);
        assert!(ligado(Perfil::Energica) < ligado(Perfil::Neutra));
        assert!(ligado(Perfil::Melancolica) >= ligado(Perfil::Neutra));
    }

    #[test]
    fn frase_mais_lenta_nas_pontas_e_relogio_nos_dois_sentidos() {
        let p = exemplo("escala").unwrap();
        let e = interpretar(&p, Perfil::Melancolica, 0.5);
        let r = &e.relogio;
        let dur = |t: f64| r.segundos(t + 1.0) - r.segundos(t);
        // 1ª frase (0 a 16): ponta mais lenta que o meio
        assert!(dur(0.0) > dur(7.5) * 1.05, "{} {}", dur(0.0), dur(7.5));
        // último tempo da frase com respiração
        assert!(dur(15.0) > dur(14.0));
        for t in [0.0, 3.3, 15.9, p.fim() - 0.5, p.fim() + 2.0] {
            assert!((r.tempo_em(r.segundos(t)) - t).abs() < 1e-6, "{t}");
        }
    }

    #[test]
    fn pontuado_marcado_ou_suavizado() {
        // semínima pontuada + colcheia, duas vezes
        let n = |inicio, duracao| Nota { inicio, duracao, tecla: 40, dinamica: 0.5, ..Nota::default() };
        let p = Partitura { notas: vec![n(0.0, 1.5), n(1.5, 0.5), n(2.0, 1.5), n(3.5, 0.5)], ..Partitura::default() };
        let energica = interpretar(&p, Perfil::Energica, 0.0);
        let melancolica = interpretar(&p, Perfil::Melancolica, 0.0);
        assert!(energica.notas[1].adianto_s < 0.0, "a curta atrasa");
        assert!(melancolica.notas[1].adianto_s > 0.0, "a curta adianta");
        assert_eq!(energica.notas[0].adianto_s, 0.0);
    }
}
