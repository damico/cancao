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

//! Sintetizador polifônico de piano. Aqui fica o que é do instrumento e
//! não depende de como a corda é simulada:
//!
//! - teclas e abafadores: o abafador encosta na corda quando a tecla é
//!   solta, a menos que um pedal o segure; o contato pode ser parcial
//!   (meio pedal)
//! - pedais: sustain, sostenuto, una corda
//! - vozes: alocação, ruído do martelo, posição estéreo
//! - cavalete -> tampo -> estéreo
//!
//! A corda em si é simulada por um de dois modelos (ver [`Modelo`]).

use std::f32::consts::PI;

use crate::corda::{self, Corda, NUM_TECLAS};
use crate::eventos::Pedal;
use crate::guia::{self, Dispersao, VozGuia};
use crate::modal::VozModal;
use crate::tampo::{self, Tampo};

pub const MAX_VOZES: usize = 32;
pub const MAX_UNISSONO: usize = 3;
/// Quadros gerados por chamada (o tampo exige blocos deste tamanho).
pub const BLOCO: usize = tampo::BLOCO;

/// ponto de batida do martelo / L
pub const POS_MARTELO: f32 = 0.12;
/// diferenças de afinação entre as cordas do uníssono, em cents; vêm de
/// diferenças de tensão: dT/T = 2 df/f
pub const DESAFINO_UNISSONO: [f32; MAX_UNISSONO] = [0.0, 0.9, -0.6];
/// corda não atingida pelo martelo com una corda: vibra por simpatia
pub const SIMPATIA_UNA_CORDA: f32 = 0.12;
/// s, com o abafador totalmente encostado
pub const TAU_ABAFADO: f32 = 0.08;
/// do F#6 para cima não há abafadores
const TECLA_SEM_ABAFADOR: usize = 70;

/// Como a corda é simulada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modelo {
    /// soma de senoides: imita o resultado da física
    Modal,
    /// guia de onda digital: simula a onda viajando na corda
    Guia,
}

impl Modelo {
    pub fn nome(self) -> &'static str {
        match self {
            Modelo::Modal => "modal",
            Modelo::Guia => "guia de onda",
        }
    }
}

#[derive(Clone, Copy)]
enum Corpo {
    Modal(VozModal),
    Guia(VozGuia),
}

#[derive(Clone, Copy)]
struct Voz {
    ativa: bool,
    tecla: usize,
    /// contato do abafador com a corda: 0 = levantado, 1 = encostado
    contato: f32,
    idade: u32,
    corpo: Corpo,
    // "thump" do martelo
    ruido_amp: f32,
    ruido_dec: f32,
    ruido_lp: f32,
    rng: u32,
    // pan estéreo
    ganho_e: f32,
    ganho_d: f32,
}

impl Default for Voz {
    fn default() -> Self {
        Voz {
            ativa: false,
            tecla: 0,
            contato: 0.0,
            idade: 0,
            corpo: Corpo::Modal(VozModal::default()),
            ruido_amp: 0.0,
            ruido_dec: 0.0,
            ruido_lp: 0.0,
            rng: 0,
            ganho_e: 0.0,
            ganho_d: 0.0,
        }
    }
}

pub struct Synth {
    taxa: f32,
    cordas: Vec<Corda>,
    /// filtro de dispersão de cada tecla para o guia de onda
    dispersao: Vec<Dispersao>,
    vozes: Box<[Voz; MAX_VOZES]>,
    relogio: u32,
    /// modelo usado nas próximas notas
    pub modelo: Modelo,
    pub volume: f32,
    tampo: Option<Tampo>,
    pub tampo_ligado: bool,
    pressionadas: [bool; NUM_TECLAS],
    /// abafadores presos pelo sostenuto
    sostenuto_preso: [bool; NUM_TECLAS],
    sustain: f32,
    sostenuto: bool,
    una_corda: f32,
    /// quanto do som direto das cordas misturar ao do tampo
    pub mix_seco: f32,
    pub ganho_tampo: f32,
}

impl Synth {
    pub fn new(taxa: f32, fator_tensao: f64, tampo: Option<Tampo>) -> Self {
        let cordas = corda::cordas(fator_tensao);
        Synth {
            taxa,
            dispersao: cordas.iter().map(|c| guia::projetar_dispersao(c, taxa)).collect(),
            cordas,
            // via Vec: um array de vozes no stack passaria de 800 KB
            vozes: vec![Voz::default(); MAX_VOZES].into_boxed_slice().try_into().ok().unwrap(),
            relogio: 0,
            modelo: Modelo::Modal,
            volume: 0.8,
            tampo_ligado: tampo.is_some(),
            tampo,
            pressionadas: [false; NUM_TECLAS],
            sostenuto_preso: [false; NUM_TECLAS],
            sustain: 0.0,
            sostenuto: false,
            una_corda: 0.0,
            mix_seco: 0.2,
            ganho_tampo: 0.18,
        }
    }

    pub fn com_modelo(mut self, modelo: Modelo) -> Self {
        self.modelo = modelo;
        self
    }

    pub fn cordas(&self) -> &[Corda] {
        &self.cordas
    }

    pub fn tem_tampo(&self) -> bool {
        self.tampo.is_some()
    }

    /// posição atual de um pedal (0..1)
    pub fn pedal(&self, p: Pedal) -> f32 {
        match p {
            Pedal::Sustain => self.sustain,
            Pedal::Sostenuto => self.sostenuto as u8 as f32,
            Pedal::UnaCorda => self.una_corda,
        }
    }

    pub fn vozes_ativas(&self) -> usize {
        self.vozes.iter().filter(|v| v.ativa).count()
    }

    fn achar_voz(&self, tecla: usize) -> usize {
        // re-ataque na mesma corda
        if let Some(i) = self.vozes.iter().position(|v| v.ativa && v.tecla == tecla) {
            return i;
        }
        if let Some(i) = self.vozes.iter().position(|v| !v.ativa) {
            return i;
        }
        // rouba a voz mais antiga
        (0..MAX_VOZES).min_by_key(|&i| self.vozes[i].idade).unwrap()
    }

    /// velocidade 0..1
    pub fn nota_on(&mut self, tecla: usize, vel: f32) {
        if tecla >= NUM_TECLAS {
            return;
        }
        let vel = vel.clamp(0.05, 1.0);
        self.relogio += 1;
        self.pressionadas[tecla] = true;
        let taxa = self.taxa;
        let c = &self.cordas[tecla];
        let f = c.f as f32;

        // compensa a perda de volume percebido nos agudos curtos
        let brilho = if f > 440.0 { (f / 440.0).powf(0.4) } else { 1.0 };
        let ganho = 0.35 * vel.powf(1.6) * brilho;
        let corpo = match self.modelo {
            Modelo::Modal => Corpo::Modal(VozModal::new(c, vel, self.una_corda, ganho, taxa)),
            Modelo::Guia => Corpo::Guia(VozGuia::new(c, self.dispersao[tecla], vel, self.una_corda, ganho, taxa)),
        };

        let i_voz = self.achar_voz(tecla);
        let v = &mut self.vozes[i_voz];
        *v = Voz {
            ativa: true,
            tecla,
            contato: 0.0,
            idade: self.relogio,
            corpo,
            rng: 0x9E37_79B9 ^ (tecla as u32 * 7919).wrapping_add(self.relogio),
            ruido_amp: 0.04 * vel * vel,
            ruido_dec: (-1.0 / (0.004 * taxa)).exp(),
            ..Voz::default()
        };

        // pan: graves à esquerda, agudos à direita (como o pianista ouve)
        let pan = (tecla as f32 / (NUM_TECLAS - 1) as f32 - 0.5) * 1.2;
        let ang = (pan + 1.0) * PI / 4.0;
        v.ganho_e = ang.cos();
        v.ganho_d = ang.sin();
    }

    /// Contato do abafador com a corda de uma tecla (0 = livre, 1 = abafando).
    fn contato(&self, tecla: usize) -> f32 {
        if tecla >= TECLA_SEM_ABAFADOR || self.pressionadas[tecla] || self.sostenuto_preso[tecla] {
            return 0.0;
        }
        // meio pedal: até 25% o abafador encosta inteiro, a partir de 75% fica
        // levantado; entre os dois só roça a corda e abafa devagar
        ((0.75 - self.sustain) / 0.5).clamp(0.0, 1.0)
    }

    /// Recalcula o decaimento das vozes cujo abafador mudou de posição.
    /// Se o abafador volta a levantar, a corda segue com o decaimento natural.
    fn atualizar_abafadores(&mut self) {
        let contatos: [f32; NUM_TECLAS] = std::array::from_fn(|k| self.contato(k));
        let taxa = self.taxa;
        for v in self.vozes.iter_mut().filter(|v| v.ativa) {
            let c = contatos[v.tecla];
            if c == v.contato {
                continue;
            }
            v.contato = c;
            match &mut v.corpo {
                Corpo::Modal(m) => m.abafador(c, taxa),
                Corpo::Guia(g) => g.abafador(c, taxa),
            }
        }
    }

    pub fn nota_off(&mut self, tecla: usize) {
        if tecla < NUM_TECLAS {
            self.pressionadas[tecla] = false;
            self.atualizar_abafadores();
        }
    }

    pub fn mudar_pedal(&mut self, pedal: Pedal, valor: f32) {
        let valor = valor.clamp(0.0, 1.0);
        match pedal {
            Pedal::Sustain => self.sustain = valor,
            // vale para as próximas notas: a mecânica desloca antes do golpe
            Pedal::UnaCorda => self.una_corda = valor,
            Pedal::Sostenuto => {
                let desce = valor >= 0.5;
                if desce && !self.sostenuto {
                    // prende os abafadores que estão levantados agora, inclusive
                    // os levantados pelo sustain (como num piano de cauda)
                    for k in 0..NUM_TECLAS {
                        self.sostenuto_preso[k] = self.contato(k) == 0.0 && k < TECLA_SEM_ABAFADOR;
                    }
                } else if !desce {
                    self.sostenuto_preso = [false; NUM_TECLAS];
                }
                self.sostenuto = desce;
            }
        }
        self.atualizar_abafadores();
    }

    /// Solta todas as teclas e pedais.
    pub fn soltar_tudo(&mut self) {
        self.pressionadas = [false; NUM_TECLAS];
        self.sostenuto_preso = [false; NUM_TECLAS];
        self.sustain = 0.0;
        self.sostenuto = false;
        self.una_corda = 0.0;
        self.atualizar_abafadores();
    }

    /// Gera um bloco estéreo intercalado (-1..1).
    pub fn render_bloco(&mut self, saida: &mut [f32; 2 * BLOCO]) {
        let mut mono = [0f32; BLOCO];
        let mut cavalete = [0f32; BLOCO];
        saida.fill(0.0);

        for v in self.vozes.iter_mut().filter(|v| v.ativa) {
            render_voz(v, &mut mono);
            for t in 0..BLOCO {
                saida[2 * t] += mono[t] * v.ganho_e;
                saida[2 * t + 1] += mono[t] * v.ganho_d;
                cavalete[t] += mono[t];
            }
        }

        // todas as cordas excitam o mesmo cavalete -> tampo -> estéreo
        if let Some(tampo) = self.tampo.as_mut().filter(|_| self.tampo_ligado) {
            let mut irradiado = [0f32; 2 * BLOCO];
            tampo.processar(&cavalete, &mut irradiado);
            for (s, t) in saida.iter_mut().zip(irradiado) {
                *s = *s * self.mix_seco + t * self.ganho_tampo;
            }
        }

        // saturação suave para muitos acordes simultâneos
        for s in saida.iter_mut() {
            let x = (*s * self.volume).clamp(-3.0, 3.0);
            *s = x * (27.0 + x * x) / (27.0 + 9.0 * x * x);
        }
    }
}

fn render_voz(v: &mut Voz, mono: &mut [f32; BLOCO]) {
    let energia = match &mut v.corpo {
        Corpo::Modal(m) => m.render(mono),
        Corpo::Guia(g) => g.render(mono),
    };
    if v.ruido_amp > 1e-6 {
        for m in mono.iter_mut() {
            v.rng = v.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let r = v.rng as i32 as f32 * (1.0 / 2_147_483_648.0);
            v.ruido_lp += 0.25 * (r - v.ruido_lp);
            *m += v.ruido_lp * v.ruido_amp;
            v.ruido_amp *= v.ruido_dec;
        }
    }
    if energia < 1e-5 && v.ruido_amp < 1e-6 {
        v.ativa = false;
    }
}
