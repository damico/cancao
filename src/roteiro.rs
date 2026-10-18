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

//! Roteiro: uma performance pronta, como lista de eventos com horário.
//!
//! É o que o player vai produzir a partir de uma partitura. Por enquanto
//! há roteiros fixos para demonstração e teste.

use crate::corda::{NUM_TECLAS, tecla_de_nome};
use crate::eventos::{Evento, EventoAgendado, Pedal};

pub struct Roteiro {
    taxa: f64,
    pub eventos: Vec<EventoAgendado>,
    /// textos para mostrar durante a execução
    pub marcos: Vec<(u64, String)>,
    /// duração total em quadros, incluindo o decaimento final
    pub duracao: u64,
}

impl Roteiro {
    pub fn new(taxa: u32) -> Self {
        Roteiro { taxa: taxa as f64, eventos: Vec::new(), marcos: Vec::new(), duracao: 0 }
    }

    pub fn quadro(&self, t_s: f64) -> u64 {
        (t_s * self.taxa).round() as u64
    }

    pub fn evento(&mut self, t_s: f64, evento: Evento) -> &mut Self {
        let quadro = self.quadro(t_s);
        self.eventos.push(EventoAgendado { quadro, evento });
        self.duracao = self.duracao.max(quadro);
        self
    }

    /// Abaixa a tecla em `t_s` e solta `dur_s` segundos depois.
    pub fn nota(&mut self, t_s: f64, tecla: usize, vel: f32, dur_s: f64) -> &mut Self {
        let tecla = tecla as u8;
        self.evento(t_s, Evento::NotaOn { tecla, vel })
            .evento(t_s + dur_s, Evento::NotaOff { tecla })
    }

    pub fn pedal(&mut self, t_s: f64, pedal: Pedal, valor: f32) -> &mut Self {
        self.evento(t_s, Evento::Pedal { pedal, valor })
    }

    pub fn marco(&mut self, t_s: f64, texto: &str) -> &mut Self {
        let q = self.quadro(t_s);
        self.marcos.push((q, texto.to_string()));
        self
    }

    /// Tempo extra no fim para o som decair.
    pub fn cauda(&mut self, s: f64) -> &mut Self {
        self.duracao += self.quadro(s);
        self
    }

    /// Eventos deslocados para começar no quadro `inicio`.
    pub fn a_partir_de(&self, inicio: u64) -> impl Iterator<Item = EventoAgendado> + '_ {
        self.eventos.iter().map(move |e| EventoAgendado { quadro: e.quadro + inicio, ..*e })
    }
}

fn t(nome: &str) -> usize {
    tecla_de_nome(nome).expect("nota válida")
}

/// As 88 teclas em sequência, depois um acorde de Dó maior abafado.
pub fn teclas(taxa: u32) -> Roteiro {
    let mut r = Roteiro::new(taxa);
    let passo = 0.35;
    for k in 0..NUM_TECLAS {
        r.nota(k as f64 * passo, k, 0.7, 3.0 * passo);
    }
    let t_acorde = passo * NUM_TECLAS as f64 + 1.0;
    for n in ["C2", "C3", "E4", "G4", "C5"] {
        r.evento(t_acorde, Evento::NotaOn { tecla: t(n) as u8, vel: 0.8 });
    }
    r.evento(t_acorde + 3.0, Evento::SoltarTudo).cauda(2.0);
    r
}

/// Notas uma após a outra; cada uma dura `dur_s`, ou fica segurada até o
/// fim se `None`.
pub fn notas(taxa: u32, teclas: &[usize], vel: f32, intervalo_s: f64, dur_s: Option<f64>, fim_s: f64) -> Roteiro {
    let mut r = Roteiro::new(taxa);
    let soltar = intervalo_s * teclas.len() as f64 + fim_s;
    for (i, &k) in teclas.iter().enumerate() {
        let inicio = i as f64 * intervalo_s;
        r.nota(inicio, k, vel, dur_s.unwrap_or(soltar - inicio));
    }
    r.duracao = r.duracao.max(r.quadro(soltar));
    r
}

/// Arpejo de Dó maior com notas soltas (destacadas).
fn frase(r: &mut Roteiro, t0: f64) -> f64 {
    let notas = ["C4", "E4", "G4", "C5", "E5", "G5", "C6"];
    for (i, n) in notas.iter().enumerate() {
        r.nota(t0 + i as f64 * 0.22, t(n), 0.65, 0.18);
    }
    t0 + notas.len() as f64 * 0.22
}

/// Demonstração dos três pedais, em cinco trechos.
pub fn pedais(taxa: u32) -> Roteiro {
    let mut r = Roteiro::new(taxa);

    r.marco(0.0, "1/5 sem pedal: cada nota é abafada ao soltar a tecla");
    frase(&mut r, 0.0);

    r.marco(3.0, "2/5 sustain: as mesmas notas soltas continuam soando");
    r.pedal(2.95, Pedal::Sustain, 1.0);
    let fim = frase(&mut r, 3.0);
    r.pedal(fim + 2.0, Pedal::Sustain, 0.0);
    r.marco(fim + 2.0, "    pedal solto: tudo abafa");

    r.marco(7.5, "3/5 meio pedal (50%): abafadores roçando, as notas morrem devagar");
    r.pedal(7.45, Pedal::Sustain, 0.5);
    let fim = frase(&mut r, 7.5);
    r.pedal(fim + 2.5, Pedal::Sustain, 0.0);

    let acorde = ["C3", "G3", "E4", "C5"];
    r.marco(12.5, "4/5 acorde normal...");
    for n in acorde {
        r.nota(12.5, t(n), 0.8, 2.2);
    }
    r.marco(15.5, "    ...e com una corda: mais suave e mais escuro");
    r.pedal(15.4, Pedal::UnaCorda, 1.0);
    for n in acorde {
        r.nota(15.5, t(n), 0.8, 2.2);
    }
    r.pedal(18.0, Pedal::UnaCorda, 0.0);

    r.marco(19.0, "5/5 sostenuto: o baixo fica preso, a melodia por cima continua solta");
    r.nota(19.0, t("C2"), 0.8, 0.5).nota(19.0, t("C3"), 0.8, 0.5);
    r.pedal(19.3, Pedal::Sostenuto, 1.0);
    let fim = frase(&mut r, 19.8);
    r.pedal(fim + 1.5, Pedal::Sostenuto, 0.0);
    r.marco(fim + 1.5, "    sostenuto solto: o baixo abafa");
    r.cauda(1.5);
    r
}
