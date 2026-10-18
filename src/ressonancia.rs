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

//! Ressonância simpática: as cordas que ninguém tocou, mas que estão livres.
//!
//! Todas as cordas do piano estão presas ao mesmo cavalete. Quando o abafador
//! de uma tecla está levantado (sustain, tecla abaixada, sostenuto, ou as
//! teclas do F#6 para cima, que não têm abafador), a corda dela fica livre e
//! o movimento do cavalete a faz vibrar, principalmente quando algum parcial
//! dela coincide com algo que está soando. Isso dá o "halo" do piano com
//! pedal.
//!
//! Uma corda simplificada por tecla (guia de onda de uma corda só, sem a
//! cascata de dispersão, para caber no tempo real com as 88 livres), afinada
//! e com perdas como a corda de verdade. A entrada é o sinal do cavalete (a
//! soma das notas tocadas); a saída volta para o cavalete e o tampo. As
//! ressoadoras não excitam umas às outras (sem realimentação: sempre estável).
//! Uma tecla que está soando como voz não ressoa à parte (seriam as mesmas
//! cordas duas vezes).

use crate::corda::{Corda, NUM_TECLAS};
use crate::guia::{ajustar_perdas, sintonia, tau_corda, Dispersao};
use crate::synth::{BLOCO, TAU_ABAFADO};

/// Quanto do movimento do cavalete entra em cada corda livre.
const ACOPLAMENTO: f32 = 0.0002;
/// Abaixo disto a corda livre é considerada parada.
const SILENCIO: f32 = 1e-6;

struct CordaLivre {
    linha: Vec<f32>,
    pos: usize,
    f: f32,
    /// perdas: y = b*x − a*y₋₁, com b = g*(1 + a)
    a: f32,
    b: f32,
    g_natural: f32,
    lp_y1: f32,
    /// passa-tudo fracionário
    eta: f32,
    ap_x1: f32,
    ap_y1: f32,
    /// contato do abafador (0 livre, 1 abafando)
    contato: f32,
    /// pico recente da saída, para saber se ainda vibra
    nivel: f32,
}

impl CordaLivre {
    fn new(c: &Corda, taxa: f32) -> Self {
        let f = c.parcial(1) as f32;
        let (a, g) = ajustar_perdas(f, tau_corda(c), taxa);
        let (n, eta) = sintonia(f, a, Dispersao::default(), taxa);
        CordaLivre {
            linha: vec![0.0; n],
            pos: 0,
            f,
            a,
            b: g * (1.0 + a),
            g_natural: g,
            lp_y1: 0.0,
            eta,
            ap_x1: 0.0,
            ap_y1: 0.0,
            contato: 1.0,
            nivel: 0.0,
        }
    }

    fn abafador(&mut self, contato: f32) {
        if contato != self.contato {
            self.contato = contato;
            // perda extra por volta (1/f segundos) com o abafador encostado
            let fator = (-contato / (TAU_ABAFADO * self.f)).exp();
            self.b = self.g_natural * fator * (1.0 + self.a);
        }
    }

    fn zerar(&mut self) {
        self.linha.fill(0.0);
        (self.lp_y1, self.ap_x1, self.ap_y1, self.nivel) = (0.0, 0.0, 0.0, 0.0);
    }

    /// Soma a saída desta corda em `saida`, excitada por `cavalete`.
    fn processar(&mut self, cavalete: &[f32; BLOCO], saida: &mut [f32; BLOCO]) {
        let mut pico = 0f32;
        for (e, s) in cavalete.iter().zip(saida.iter_mut()) {
            let x = self.linha[self.pos];
            let lp = self.b * x - self.a * self.lp_y1;
            self.lp_y1 = lp;
            let ap = self.eta * lp + self.ap_x1 - self.eta * self.ap_y1;
            self.ap_x1 = lp;
            self.ap_y1 = ap;
            let v = ap + e * ACOPLAMENTO;
            self.linha[self.pos] = v;
            self.pos += 1;
            if self.pos == self.linha.len() {
                self.pos = 0;
            }
            *s += v;
            pico = pico.max(v.abs());
        }
        // decai devagar entre blocos: no grave a onda passa uma vez por volta
        self.nivel = pico.max(self.nivel * 0.9);
    }
}

pub struct Ressonancia {
    cordas: Vec<CordaLivre>,
}

impl Ressonancia {
    pub fn new(cordas: &[Corda], taxa: f32) -> Self {
        Ressonancia { cordas: cordas.iter().map(|c| CordaLivre::new(c, taxa)).collect() }
    }

    /// Soma em `saida` o som das cordas livres excitadas pelo `cavalete`.
    /// `contatos`: abafador de cada tecla (0 livre); `tocando`: teclas que já
    /// soam como voz (não ressoam à parte).
    pub fn processar(&mut self, cavalete: &[f32; BLOCO], contatos: &[f32; NUM_TECLAS], tocando: &[bool; NUM_TECLAS], saida: &mut [f32; BLOCO]) {
        for (k, c) in self.cordas.iter_mut().enumerate() {
            if tocando[k] {
                if c.nivel > 0.0 {
                    c.zerar();
                }
                continue;
            }
            c.abafador(contatos[k]);
            let livre = contatos[k] < 1.0;
            if livre || c.nivel > SILENCIO {
                c.processar(cavalete, saida);
            }
        }
    }

    /// Cordas sendo processadas agora (para medir o custo).
    pub fn ativas(&self) -> usize {
        self.cordas.iter().filter(|c| c.contato < 1.0 || c.nivel > SILENCIO).count()
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::corda::{self, tecla_de_nome};
    use std::f32::consts::PI;

    /// energia de cada corda livre depois de excitar o cavalete com uma senoide
    fn resposta(freq: f32) -> Vec<f32> {
        let taxa = 48000.0;
        let cordas = corda::cordas(1.0);
        let mut r = Ressonancia::new(&cordas, taxa);
        let contatos = [0.0f32; NUM_TECLAS];
        let tocando = [false; NUM_TECLAS];
        let mut t = 0usize;
        for _ in 0..(taxa as usize / BLOCO) {
            let mut cav = [0f32; BLOCO];
            for x in cav.iter_mut() {
                *x = (2.0 * PI * freq * t as f32 / taxa).sin();
                t += 1;
            }
            let mut lixo = [0f32; BLOCO];
            r.processar(&cav, &contatos, &tocando, &mut lixo);
        }
        r.cordas.iter().map(|c| c.linha.iter().map(|x| x * x).sum::<f32>() / c.linha.len() as f32).collect()
    }

    #[test]
    fn corda_afinada_com_o_cavalete_ressoa_mais() {
        // excitando em A4 (440 Hz): ressoam A4, A3 (2º parcial) e A2 (4º),
        // bem mais que uma corda sem parcial perto de 440 Hz (C#4)
        let e = resposta(cordas_f("A4"));
        let k = |n| tecla_de_nome(n).unwrap();
        assert!(e[k("A4")] > 100.0 * e[k("C#4")], "A4 {} vs C#4 {}", e[k("A4")], e[k("C#4")]);
        assert!(e[k("A3")] > 10.0 * e[k("C#4")], "A3 {} vs C#4 {}", e[k("A3")], e[k("C#4")]);
        assert!(e[k("A2")] > 10.0 * e[k("C#4")], "A2 {} vs C#4 {}", e[k("A2")], e[k("C#4")]);
    }

    fn cordas_f(nome: &str) -> f32 {
        corda::cordas(1.0)[tecla_de_nome(nome).unwrap()].parcial(1) as f32
    }

    #[test]
    fn abafada_nao_ressoa() {
        let taxa = 48000.0;
        let cordas = corda::cordas(1.0);
        let mut r = Ressonancia::new(&cordas, taxa);
        let contatos = [1.0f32; NUM_TECLAS];
        let tocando = [false; NUM_TECLAS];
        let mut cav = [0f32; BLOCO];
        cav[0] = 1.0;
        let mut s = [0f32; BLOCO];
        r.processar(&cav, &contatos, &tocando, &mut s);
        assert!(s.iter().all(|&x| x == 0.0));
        assert_eq!(r.ativas(), 0);
    }
}
