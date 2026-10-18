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

//! Transporte: tocar, pausar, parar e saltar.
//!
//! O relógio do áudio é a referência: no play, a posição da execução é
//! ancorada num quadro do motor, e a interface só lê onde o motor está. Os
//! eventos vão para o motor aos poucos, até [`ANTECEDENCIA_S`] à frente: uma
//! obra inteira não cabe na fila de eventos de uma vez.

use crate::eventos::{Evento, EventoAgendado, Pedal};

/// Quanto à frente os eventos são enviados ao motor (s).
const ANTECEDENCIA_S: f64 = 2.0;
/// Folga entre apertar play e o primeiro som, para nada chegar atrasado (s).
const FOLGA_S: f64 = 0.08;

pub struct Transporte {
    /// eventos da execução, em quadros desde o início da peça, em ordem
    eventos: Vec<EventoAgendado>,
    taxa: f64,
    duracao_s: f64,
    tocando: bool,
    /// posição (s da execução) quando parado ou pausado
    pos_s: f64,
    /// no play: (quadro do motor, s da execução) que se correspondem
    ancora: (u64, f64),
    proximo: usize,
}

impl Transporte {
    pub fn new(mut eventos: Vec<EventoAgendado>, duracao_quadros: u64, taxa: u32) -> Self {
        eventos.sort_by_key(|e| e.quadro);
        Transporte {
            eventos,
            taxa: taxa as f64,
            duracao_s: duracao_quadros as f64 / taxa as f64,
            tocando: false,
            pos_s: 0.0,
            ancora: (0, 0.0),
            proximo: 0,
        }
    }

    pub fn tocando(&self) -> bool {
        self.tocando
    }

    /// Posição atual da execução (s), dado o quadro atual do motor.
    pub fn agora_s(&self, quadro: u64) -> f64 {
        if !self.tocando {
            return self.pos_s;
        }
        let (q, s) = self.ancora;
        (s + (quadro as f64 - q as f64) / self.taxa).max(s)
    }

    /// `envia` devolve false se a fila do motor estiver cheia.
    pub fn tocar(&mut self, quadro: u64, envia: &mut dyn FnMut(EventoAgendado) -> bool) {
        if self.tocando {
            return;
        }
        if self.pos_s >= self.duracao_s {
            self.pos_s = 0.0;
        }
        let inicio = quadro + (FOLGA_S * self.taxa) as u64;
        self.ancora = (inicio, self.pos_s);
        let pos_quadro = (self.pos_s * self.taxa) as u64;
        self.proximo = self.eventos.partition_point(|e| e.quadro < pos_quadro);
        // saltando para o meio de um trecho com pedal: reconstrói o sustain
        let sustain = self.eventos[..self.proximo]
            .iter()
            .rev()
            .find_map(|e| match e.evento {
                Evento::Pedal { pedal: Pedal::Sustain, valor } => Some(valor),
                _ => None,
            })
            .unwrap_or(0.0);
        envia(EventoAgendado { quadro: inicio, evento: Evento::Pedal { pedal: Pedal::Sustain, valor: sustain } });
        self.tocando = true;
        self.alimentar(quadro, envia);
    }

    /// Para o som e guarda a posição.
    pub fn pausar(&mut self, quadro: u64, envia: &mut dyn FnMut(EventoAgendado) -> bool) {
        if self.tocando {
            self.pos_s = self.agora_s(quadro).min(self.duracao_s);
            self.tocando = false;
        }
        envia(EventoAgendado::agora(Evento::CancelarAgendados));
        envia(EventoAgendado::agora(Evento::SoltarTudo));
    }

    pub fn parar(&mut self, quadro: u64, envia: &mut dyn FnMut(EventoAgendado) -> bool) {
        self.pausar(quadro, envia);
        self.pos_s = 0.0;
    }

    /// Vai para a posição `s` (segundos da execução), continuando a tocar se
    /// estava tocando.
    pub fn saltar(&mut self, s: f64, quadro: u64, envia: &mut dyn FnMut(EventoAgendado) -> bool) {
        let tocava = self.tocando;
        self.pausar(quadro, envia);
        self.pos_s = s.clamp(0.0, self.duracao_s);
        if tocava {
            self.tocar(quadro, envia);
        }
    }

    /// Envia os eventos dos próximos segundos; para no fim da peça.
    pub fn alimentar(&mut self, quadro: u64, envia: &mut dyn FnMut(EventoAgendado) -> bool) {
        if !self.tocando {
            return;
        }
        if self.agora_s(quadro) >= self.duracao_s {
            self.tocando = false;
            self.pos_s = self.duracao_s;
            return;
        }
        let (q0, s0) = self.ancora;
        let ate_s = self.agora_s(quadro) + ANTECEDENCIA_S;
        while let Some(e) = self.eventos.get(self.proximo) {
            let s = e.quadro as f64 / self.taxa;
            if s > ate_s {
                break;
            }
            let no_motor = q0 + ((s - s0).max(0.0) * self.taxa) as u64;
            if !envia(EventoAgendado { quadro: no_motor, evento: e.evento }) {
                break; // fila cheia: tenta de novo no próximo quadro
            }
            self.proximo += 1;
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use std::cell::RefCell;

    const TAXA: u32 = 1000; // 1 quadro = 1 ms, para facilitar as contas

    fn nota(ms: u64, tecla: u8) -> EventoAgendado {
        EventoAgendado { quadro: ms, evento: Evento::NotaOn { tecla, vel: 0.5 } }
    }

    fn transporte() -> Transporte {
        let mut ev: Vec<EventoAgendado> = (0..10).map(|i| nota(i * 1000, i as u8)).collect();
        ev.push(EventoAgendado { quadro: 2500, evento: Evento::Pedal { pedal: Pedal::Sustain, valor: 1.0 } });
        Transporte::new(ev, 10_000, TAXA)
    }

    #[test]
    fn toca_so_os_proximos_segundos_e_alimenta_depois() {
        let mut t = transporte();
        let enviados = RefCell::new(Vec::new());
        let mut envia = |e: EventoAgendado| {
            enviados.borrow_mut().push(e);
            true
        };
        t.tocar(5000, &mut envia);
        // pedal inicial (solto) + notas em 0, 1 e 2 s (antecedência de 2 s)
        assert_eq!(enviados.borrow().len(), 4);
        assert_eq!(enviados.borrow()[1].quadro, 5000 + 80); // ancorado com a folga
        t.alimentar(5000 + 80 + 3000, &mut envia);
        assert!(enviados.borrow().len() > 4);
    }

    #[test]
    fn pausar_guarda_a_posicao_e_cancela() {
        let mut t = transporte();
        let enviados = RefCell::new(Vec::new());
        let mut envia = |e: EventoAgendado| {
            enviados.borrow_mut().push(e);
            true
        };
        t.tocar(0, &mut envia);
        t.pausar(80 + 3500, &mut envia);
        assert!(!t.tocando());
        assert!((t.agora_s(999_999) - 3.5).abs() < 1e-9);
        assert!(enviados.borrow().iter().any(|e| e.evento == Evento::CancelarAgendados));
    }

    #[test]
    fn saltar_para_dentro_do_pedal_reconstroi_o_sustain() {
        let mut t = transporte();
        let enviados = RefCell::new(Vec::new());
        let mut envia = |e: EventoAgendado| {
            enviados.borrow_mut().push(e);
            true
        };
        t.saltar(4.0, 0, &mut envia);
        t.tocar(0, &mut envia);
        let enviados = enviados.borrow();
        let pedal = enviados.iter().find(|e| matches!(e.evento, Evento::Pedal { .. })).unwrap();
        assert_eq!(pedal.evento, Evento::Pedal { pedal: Pedal::Sustain, valor: 1.0 });
        // a primeira nota enviada é a de 4 s, no instante do play
        let primeira = enviados.iter().find(|e| matches!(e.evento, Evento::NotaOn { .. })).unwrap();
        assert_eq!(primeira.evento, Evento::NotaOn { tecla: 4, vel: 0.5 });
    }

    #[test]
    fn para_sozinho_no_fim() {
        let mut t = transporte();
        let mut envia = |_| true;
        t.tocar(0, &mut envia);
        t.alimentar(80 + 10_500, &mut envia);
        assert!(!t.tocando());
    }

    #[test]
    fn fila_cheia_tenta_de_novo() {
        let mut t = transporte();
        let mut cabe = 2;
        let mut envia = |_| {
            cabe -= 1;
            cabe >= 0
        };
        t.tocar(0, &mut envia);
        let antes = t.proximo;
        let mut envia = |_| true;
        t.alimentar(0, &mut envia);
        assert!(t.proximo > antes);
    }
}
