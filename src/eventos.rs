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

//! Interface entre quem toca (terminal, teclado MIDI, e depois o player
//! que lê partituras) e o instrumento. O instrumento não sabe quem está
//! tocando: só recebe eventos com o instante em que devem acontecer.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pedal {
    /// direito: levanta todos os abafadores
    Sustain,
    /// meio: segura só os abafadores levantados no instante em que desce
    Sostenuto,
    /// esquerdo: desloca a mecânica, o martelo acerta uma corda a menos
    UnaCorda,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Evento {
    /// tecla abaixada: 0..87, velocidade 0..1
    NotaOn { tecla: u8, vel: f32 },
    /// tecla solta: o abafador volta para a corda (se nenhum pedal o segura)
    NotaOff { tecla: u8 },
    /// posição do pedal: 0 = solto, 1 = no fundo (valores intermediários
    /// = meio pedal)
    Pedal { pedal: Pedal, valor: f32 },
    /// solta todas as teclas e pedais
    SoltarTudo,
    AlternarTampo,
    /// troca o modelo de corda (modal ↔ guia de onda) para as próximas notas
    AlternarModelo,
    /// descarta os eventos ainda não aplicados (pausar, saltar de posição)
    CancelarAgendados,
    /// volume geral; 1 = padrão
    Volume { valor: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EventoAgendado {
    /// instante em quadros desde o início do áudio; um instante que já
    /// passou (ex.: 0) significa "assim que possível"
    pub quadro: u64,
    pub evento: Evento,
}

impl EventoAgendado {
    pub fn agora(evento: Evento) -> Self {
        EventoAgendado { quadro: 0, evento }
    }
}

/// Traduz uma mensagem MIDI de canal (qualquer canal) para um evento.
/// Notas fora das 88 teclas e mensagens sem equivalente dão `None`.
pub fn de_midi(msg: &[u8]) -> Option<Evento> {
    let (&status, dados) = msg.split_first()?;
    let d0 = *dados.first()?;
    let d1 = dados.get(1).copied().unwrap_or(0);
    let tecla = || (21..=108).contains(&d0).then(|| d0 - 21);
    match status & 0xF0 {
        0x90 if d1 > 0 => Some(Evento::NotaOn { tecla: tecla()?, vel: d1 as f32 / 127.0 }),
        // nota ligada com velocidade 0 é, por convenção, nota desligada
        0x80 | 0x90 => Some(Evento::NotaOff { tecla: tecla()? }),
        0xB0 => {
            let pedal = |pedal| Some(Evento::Pedal { pedal, valor: d1 as f32 / 127.0 });
            match d0 {
                64 => pedal(Pedal::Sustain),
                66 => pedal(Pedal::Sostenuto),
                67 => pedal(Pedal::UnaCorda),
                // all sound off / all notes off
                120 | 123 => Some(Evento::SoltarTudo),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn notas_midi() {
        assert_eq!(de_midi(&[0x90, 69, 127]), Some(Evento::NotaOn { tecla: 48, vel: 1.0 }));
        assert_eq!(de_midi(&[0x93, 21, 64]), Some(Evento::NotaOn { tecla: 0, vel: 64.0 / 127.0 }));
        assert_eq!(de_midi(&[0x80, 108, 40]), Some(Evento::NotaOff { tecla: 87 }));
        assert_eq!(de_midi(&[0x90, 60, 0]), Some(Evento::NotaOff { tecla: 39 }));
        assert_eq!(de_midi(&[0x90, 20, 100]), None); // abaixo do A0
        assert_eq!(de_midi(&[0x90, 109, 100]), None); // acima do C8
    }

    #[test]
    fn pedais_midi() {
        let p = |pedal, valor| Some(Evento::Pedal { pedal, valor });
        assert_eq!(de_midi(&[0xB0, 64, 127]), p(Pedal::Sustain, 1.0));
        assert_eq!(de_midi(&[0xB0, 64, 0]), p(Pedal::Sustain, 0.0));
        assert_eq!(de_midi(&[0xB5, 66, 127]), p(Pedal::Sostenuto, 1.0));
        assert_eq!(de_midi(&[0xB0, 67, 127]), p(Pedal::UnaCorda, 1.0));
        assert_eq!(de_midi(&[0xB0, 123, 0]), Some(Evento::SoltarTudo));
        assert_eq!(de_midi(&[0xB0, 7, 100]), None); // volume: sem equivalente
        assert_eq!(de_midi(&[0xF8]), None); // relógio MIDI
    }
}
