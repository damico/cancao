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

//! Motor de áudio: aplica os eventos no instante certo, gera o som em
//! blocos de [`BLOCO`] quadros e mede quanto tempo cada bloco levou.
//!
//! É o mesmo código no tempo real (chamado pelo driver), na gravação em WAV
//! e no benchmark. Nada aqui aloca memória ou bloqueia depois de criado.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use crate::eventos::{Evento, EventoAgendado, Pedal};
use crate::synth::{BLOCO, Modelo, Synth, VOLUME_PADRAO};

const MAX_PENDENTES: usize = 4096;

/// Estado visível de outras threads (interface, vigia).
#[derive(Default)]
pub struct Estado {
    /// quadros já gerados
    pub relogio: AtomicU64,
    /// avança a cada chamada do driver; parado = áudio travado
    pub pulso: AtomicU64,
    pub vozes: AtomicU32,
    /// última janela de ~0,5 s, em centésimos de % do prazo do bloco
    pub carga_media: AtomicU32,
    pub carga_pico: AtomicU32,
    pub tampo_ligado: AtomicBool,
    pub tempo_real: AtomicBool,
    /// modelo das próximas notas: false = modal, true = guia de onda
    pub modelo_guia: AtomicBool,
    /// posição dos pedais em % (sustain, sostenuto, una corda)
    pub pedais: [AtomicU32; 3],
}

impl Estado {
    pub fn pedal(&self, p: Pedal) -> f32 {
        self.pedais[p as usize].load(Relaxed) as f32 / 100.0
    }
}

#[derive(Default, Clone, Copy)]
pub struct Medidor {
    pub soma_us: f64,
    pub pico_us: f64,
    pub n: u64,
    /// blocos que usaram mais de 50% do prazo
    pub acima_metade: u64,
}

impl Medidor {
    fn medir(&mut self, us: f64, prazo_us: f64) {
        self.soma_us += us;
        self.n += 1;
        self.pico_us = self.pico_us.max(us);
        if us > 0.5 * prazo_us {
            self.acima_metade += 1;
        }
    }

    pub fn media_us(&self) -> f64 {
        if self.n == 0 { 0.0 } else { self.soma_us / self.n as f64 }
    }
}

pub struct Motor {
    synth: Synth,
    pendentes: Vec<EventoAgendado>,
    relogio: u64,
    bloco: [f32; 2 * BLOCO],
    /// quadros do bloco atual já entregues (BLOCO = vazio)
    lido: usize,
    pub prazo_us: f64,
    janela: Medidor,
    pub total: Medidor,
    estado: Arc<Estado>,
}

impl Motor {
    pub fn new(synth: Synth, taxa: f32) -> Self {
        let estado = Arc::new(Estado::default());
        estado.tampo_ligado.store(synth.tampo_ligado, Relaxed);
        estado.modelo_guia.store(synth.modelo == Modelo::Guia, Relaxed);
        Motor {
            synth,
            pendentes: Vec::with_capacity(MAX_PENDENTES),
            relogio: 0,
            bloco: [0.0; 2 * BLOCO],
            lido: BLOCO,
            prazo_us: 1e6 * BLOCO as f64 / taxa as f64,
            janela: Medidor::default(),
            total: Medidor::default(),
            estado,
        }
    }

    pub fn estado(&self) -> Arc<Estado> {
        self.estado.clone()
    }

    pub fn synth(&self) -> &Synth {
        &self.synth
    }

    /// Fila cheia descarta o evento (nunca aloca no tempo real).
    pub fn agendar(&mut self, ev: EventoAgendado) {
        if ev.evento == Evento::CancelarAgendados {
            self.pendentes.clear();
        } else if self.pendentes.len() < MAX_PENDENTES {
            self.pendentes.push(ev);
        }
    }

    fn aplicar(&mut self, ev: Evento) {
        match ev {
            Evento::NotaOn { tecla, vel } => self.synth.nota_on(tecla as usize, vel),
            Evento::NotaOff { tecla } => self.synth.nota_off(tecla as usize),
            Evento::Pedal { pedal, valor } => self.synth.mudar_pedal(pedal, valor),
            Evento::SoltarTudo => self.synth.soltar_tudo(),
            Evento::AlternarTampo => {
                self.synth.tampo_ligado = self.synth.tem_tampo() && !self.synth.tampo_ligado;
                self.estado.tampo_ligado.store(self.synth.tampo_ligado, Relaxed);
            }
            Evento::CancelarAgendados => {}
            Evento::Volume { valor } => self.synth.volume = VOLUME_PADRAO * valor.clamp(0.0, 2.0),
            Evento::AlternarModelo => {
                self.synth.modelo = match self.synth.modelo {
                    Modelo::Modal => Modelo::Guia,
                    Modelo::Guia => Modelo::Modal,
                };
                self.estado.modelo_guia.store(self.synth.modelo == Modelo::Guia, Relaxed);
            }
        }
        for p in [Pedal::Sustain, Pedal::Sostenuto, Pedal::UnaCorda] {
            self.estado.pedais[p as usize].store((100.0 * self.synth.pedal(p)) as u32, Relaxed);
        }
    }

    fn gerar_bloco(&mut self) {
        let t0 = Instant::now();

        // eventos são aplicados no início do bloco que contém seu instante
        // (resolução de 2,7 ms a 48 kHz)
        let fim = self.relogio + BLOCO as u64;
        let mut i = 0;
        while i < self.pendentes.len() {
            if self.pendentes[i].quadro < fim {
                let ev = self.pendentes.remove(i).evento;
                self.aplicar(ev);
            } else {
                i += 1;
            }
        }

        self.synth.render_bloco(&mut self.bloco);
        self.relogio = fim;
        self.lido = 0;

        let us = t0.elapsed().as_secs_f64() * 1e6;
        self.janela.medir(us, self.prazo_us);
        self.total.medir(us, self.prazo_us);
        // janela de 0,5 s
        if self.janela.n as f64 * self.prazo_us >= 0.5e6 {
            let pct = |x: f64| (1e4 * x / self.prazo_us) as u32;
            self.estado.carga_media.store(pct(self.janela.media_us()), Relaxed);
            self.estado.carga_pico.store(pct(self.janela.pico_us), Relaxed);
            self.janela = Medidor::default();
        }
        self.estado.relogio.store(self.relogio, Relaxed);
        self.estado.vozes.store(self.synth.vozes_ativas() as u32, Relaxed);
    }

    /// Preenche `saida` (estéreo intercalado) de qualquer tamanho par.
    pub fn processar(&mut self, saida: &mut [f32]) {
        let mut pos = 0;
        while pos < saida.len() {
            if self.lido == BLOCO {
                self.gerar_bloco();
            }
            let n = ((saida.len() - pos) / 2).min(BLOCO - self.lido);
            saida[pos..pos + 2 * n].copy_from_slice(&self.bloco[2 * self.lido..2 * (self.lido + n)]);
            self.lido += n;
            pos += 2 * n;
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::corda::tecla_de_nome;

    const TAXA: f32 = 48000.0;

    fn motor() -> Motor {
        Motor::new(Synth::new(TAXA, 1.0, None), TAXA)
    }

    fn tocar(m: &mut Motor, segundos: f32) -> Vec<f32> {
        let mut s = vec![0f32; 2 * (segundos * TAXA) as usize];
        // tamanho de chamada que não é múltiplo do bloco, como o driver faz
        for parte in s.chunks_mut(2 * 300) {
            m.processar(parte);
        }
        s
    }

    /// pico do espectro (DFT direta) do canal esquerdo entre f0 e f1
    fn pico(s: &[f32], f0: f64, f1: f64) -> f64 {
        let x: Vec<f64> = s.iter().step_by(2).map(|&v| v as f64).collect();
        let mut melhor = (0.0, f0);
        let mut f = f0;
        while f <= f1 {
            let w = 2.0 * std::f64::consts::PI * f / TAXA as f64;
            let (re, im) = x.iter().enumerate().fold((0.0, 0.0), |(re, im), (n, &v)| {
                (re + v * (w * n as f64).cos(), im - v * (w * n as f64).sin())
            });
            let mag = re * re + im * im;
            if mag > melhor.0 {
                melhor = (mag, f);
            }
            f += 0.05;
        }
        melhor.1
    }

    #[test]
    fn a4_soa_na_frequencia_da_corda() {
        let mut m = motor();
        let a4 = tecla_de_nome("A4").unwrap();
        let esperado = m.synth().cordas()[a4].parcial(1);
        m.agendar(EventoAgendado::agora(Evento::NotaOn { tecla: a4 as u8, vel: 0.7 }));
        let s = tocar(&mut m, 1.0);
        let f = pico(&s, 435.0, 445.0);
        // as cordas do uníssono estão a 0, +0,9 e -0,6 cents: ±0,25 Hz
        assert!((f - esperado).abs() < 0.3, "medido {f} Hz, esperado {esperado} Hz");
    }

    #[test]
    fn guia_de_onda_afinado_pela_formula() {
        // com rigidez a fundamental é f √(1 + B), como no modal (o atraso
        // exato do laço é testado em guia.rs para as 88 teclas e todos os
        // parciais; aqui conferimos o som de verdade, com 3 s de resolução)
        for nome in ["A0", "A2", "A4", "C6", "C8"] {
            let tecla = tecla_de_nome(nome).unwrap();
            let mut m = Motor::new(Synth::new(TAXA, 1.0, None).com_modelo(Modelo::Guia), TAXA);
            let f = m.synth().cordas()[tecla].parcial(1);
            m.agendar(EventoAgendado::agora(Evento::NotaOn { tecla: tecla as u8, vel: 0.7 }));
            let s = tocar(&mut m, 3.0);
            let medido = pico(&s, f * 0.995, f * 1.005);
            let cents = 1200.0 * (medido / f).log2();
            // resolução da busca 0,05 Hz e uníssono a ±0,9 cent
            assert!(cents.abs() < 1.0, "{nome}: medido {medido:.2} Hz, esperado {f:.2} Hz ({cents:+.2} cents)");
        }
    }

    /// nível (dB) entre t0 e t1 segundos
    fn db(s: &[f32], t0: f32, t1: f32) -> f32 {
        20.0 * rms(s, t0, t1).log10()
    }

    #[test]
    fn guia_de_onda_decai_em_dois_estagios() {
        // cavalete: as cordas em fase perdem energia rápido, as em oposição
        // ficam; a queda no fim tem que ser bem mais lenta que no início
        for nome in ["A2", "A4", "C6"] {
            let tecla = tecla_de_nome(nome).unwrap() as u8;
            let mut m = Motor::new(Synth::new(TAXA, 1.0, None).com_modelo(Modelo::Guia), TAXA);
            m.agendar(EventoAgendado::agora(Evento::NotaOn { tecla, vel: 0.7 }));
            let s = tocar(&mut m, 9.0);
            let inicio = (db(&s, 0.05, 0.25) - db(&s, 1.0, 1.2)) / 0.95;
            let fim = (db(&s, 4.0, 4.2) - db(&s, 8.5, 8.7)) / 4.5;
            assert!(inicio > 2.5 * fim && fim > 0.0, "{nome}: {inicio:.1} dB/s no início, {fim:.1} dB/s no fim");
        }
    }

    #[test]
    fn guia_de_onda_estavel_em_todas_as_teclas() {
        // o acoplamento nunca pode criar energia
        for una in [0.0, 1.0] {
            for tecla in (0..88u8).step_by(3).chain([87]) {
                let mut m = Motor::new(Synth::new(TAXA, 1.0, None).com_modelo(Modelo::Guia), TAXA);
                pedal(&mut m, 0.0, Pedal::UnaCorda, una);
                m.agendar(EventoAgendado::agora(Evento::NotaOn { tecla, vel: 1.0 }));
                let s = tocar(&mut m, 1.5);
                assert!(s.iter().all(|x| x.is_finite()), "tecla {tecla}: valor inválido");
                let pico = |a: &[f32]| a.iter().fold(0f32, |p, x| p.max(x.abs()));
                let (antes, depois) = (pico(&s[..2 * 24000]), pico(&s[2 * 48000..]));
                assert!(depois <= antes, "tecla {tecla} (una {una}): cresceu de {antes} para {depois}");
            }
        }
    }

    #[test]
    fn guia_de_onda_grave_continua_soando() {
        // no grave a onda é um pulso curto e silêncio a cada volta; a voz não
        // pode ser desligada no silêncio entre as passagens do pulso
        let mut m = Motor::new(Synth::new(TAXA, 1.0, None).com_modelo(Modelo::Guia), TAXA);
        m.agendar(EventoAgendado::agora(Evento::NotaOn { tecla: 0, vel: 0.7 }));
        let s = tocar(&mut m, 1.5);
        let rms = |a: &[f32]| (a.iter().map(|x| x * x).sum::<f32>() / a.len() as f32).sqrt();
        let (inicio, depois) = (rms(&s[..2 * 14400]), rms(&s[2 * 48000..2 * 72000]));
        assert!(depois > 0.5 * inicio, "A0 morreu: {inicio} -> {depois}");
    }

    #[test]
    fn evento_acontece_no_instante_agendado() {
        let mut m = motor();
        m.agendar(EventoAgendado { quadro: 4800, evento: Evento::NotaOn { tecla: 40, vel: 0.8 } });
        let s = tocar(&mut m, 0.2);
        let inicio_bloco = 4800 / BLOCO * BLOCO;
        assert!(s[..2 * inicio_bloco].iter().all(|&x| x == 0.0), "som antes da hora");
        assert!(s[2 * inicio_bloco..2 * (inicio_bloco + BLOCO)].iter().any(|&x| x != 0.0));
    }

    #[test]
    fn cancelar_descarta_o_que_ainda_nao_tocou() {
        let mut m = motor();
        m.agendar(EventoAgendado { quadro: 9600, evento: Evento::NotaOn { tecla: 40, vel: 0.8 } });
        m.agendar(EventoAgendado::agora(Evento::CancelarAgendados));
        let s = tocar(&mut m, 0.5);
        assert!(s.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn volume_escala_o_som() {
        let nivel = |volume: f32| {
            let mut m = motor();
            m.agendar(EventoAgendado::agora(Evento::Volume { valor: volume }));
            m.agendar(EventoAgendado::agora(Evento::NotaOn { tecla: 40, vel: 0.3 }));
            rms(&tocar(&mut m, 0.3), 0.0, 0.3)
        };
        let (cheio, metade, mudo) = (nivel(1.0), nivel(0.5), nivel(0.0));
        assert!((metade / cheio - 0.5).abs() < 0.02, "{metade} / {cheio}");
        assert_eq!(mudo, 0.0);
    }

    #[test]
    fn abafador_corta_o_som() {
        let mut m = motor();
        m.agendar(EventoAgendado::agora(Evento::NotaOn { tecla: 30, vel: 0.8 }));
        m.agendar(EventoAgendado { quadro: 4800, evento: Evento::NotaOff { tecla: 30 } });
        let s = tocar(&mut m, 2.0);
        let rms = |a: &[f32]| (a.iter().map(|x| x * x).sum::<f32>() / a.len() as f32).sqrt();
        let antes = rms(&s[2 * 2400..2 * 4800]);
        let depois = rms(&s[2 * 48000..2 * 52800]);
        assert!(depois < antes * 1e-3, "antes {antes}, depois {depois}");
    }

    fn on(m: &mut Motor, t: f64, tecla: u8) {
        let quadro = (t * TAXA as f64) as u64;
        m.agendar(EventoAgendado { quadro, evento: Evento::NotaOn { tecla, vel: 0.8 } });
    }

    fn off(m: &mut Motor, t: f64, tecla: u8) {
        let quadro = (t * TAXA as f64) as u64;
        m.agendar(EventoAgendado { quadro, evento: Evento::NotaOff { tecla } });
    }

    fn pedal(m: &mut Motor, t: f64, pedal: Pedal, valor: f32) {
        let quadro = (t * TAXA as f64) as u64;
        m.agendar(EventoAgendado { quadro, evento: Evento::Pedal { pedal, valor } });
    }

    /// RMS entre t0 e t1 segundos
    fn rms(s: &[f32], t0: f32, t1: f32) -> f32 {
        let a = &s[2 * (t0 * TAXA) as usize..2 * (t1 * TAXA) as usize];
        (a.iter().map(|x| x * x).sum::<f32>() / a.len() as f32).sqrt()
    }

    /// nota curta (solta em 0,1 s) com o sustain numa posição; RMS em 1 s
    fn nota_curta_com_sustain(valor: f32) -> (f32, f32) {
        let mut m = motor();
        pedal(&mut m, 0.0, Pedal::Sustain, valor);
        on(&mut m, 0.0, 30);
        off(&mut m, 0.1, 30);
        let s = tocar(&mut m, 1.3);
        (rms(&s, 0.05, 0.1), rms(&s, 1.1, 1.2))
    }

    #[test]
    fn sustain_segura_a_nota_solta() {
        let (antes, sem) = nota_curta_com_sustain(0.0);
        let (_, com) = nota_curta_com_sustain(1.0);
        assert!(sem < antes * 1e-3, "sem pedal deveria abafar: {sem} vs {antes}");
        assert!(com > antes * 0.2, "com pedal deveria soar: {com} vs {antes}");
    }

    #[test]
    fn meio_pedal_abafa_devagar() {
        let rms_05 = |v| {
            let mut m = motor();
            pedal(&mut m, 0.0, Pedal::Sustain, v);
            on(&mut m, 0.0, 30);
            off(&mut m, 0.1, 30);
            rms(&tocar(&mut m, 0.6), 0.5, 0.6)
        };
        let (sem, meio, com) = (rms_05(0.0), rms_05(0.5), rms_05(1.0));
        assert!(sem < meio && meio < com * 0.5, "sem {sem}, meio {meio}, com {com}");
    }

    #[test]
    fn soltar_o_sustain_abafa() {
        let mut m = motor();
        pedal(&mut m, 0.0, Pedal::Sustain, 1.0);
        on(&mut m, 0.0, 30);
        off(&mut m, 0.1, 30);
        pedal(&mut m, 0.5, Pedal::Sustain, 0.0);
        let s = tocar(&mut m, 1.3);
        assert!(rms(&s, 1.1, 1.2) < rms(&s, 0.4, 0.5) * 1e-3);
    }

    #[test]
    fn sostenuto_prende_so_as_teclas_abaixadas() {
        // tecla abaixada quando o sostenuto desce: continua soando depois de solta
        let mut m = motor();
        on(&mut m, 0.0, 20);
        pedal(&mut m, 0.05, Pedal::Sostenuto, 1.0);
        off(&mut m, 0.1, 20);
        let s = tocar(&mut m, 1.3);
        assert!(rms(&s, 1.1, 1.2) > rms(&s, 0.05, 0.1) * 0.2, "presa deveria soar");

        // tecla abaixada depois: é abafada normalmente
        let mut m = motor();
        pedal(&mut m, 0.05, Pedal::Sostenuto, 1.0);
        on(&mut m, 0.2, 50);
        off(&mut m, 0.3, 50);
        let s = tocar(&mut m, 1.3);
        assert!(rms(&s, 1.1, 1.2) < rms(&s, 0.25, 0.3) * 1e-3, "não presa deveria abafar");
    }

    #[test]
    fn sostenuto_com_sustain_prende_tudo_que_soa() {
        // como num piano de cauda: com o sustain abaixado todos os abafadores
        // estão levantados, e o sostenuto prende todos
        let mut m = motor();
        pedal(&mut m, 0.0, Pedal::Sustain, 1.0);
        on(&mut m, 0.0, 50);
        off(&mut m, 0.02, 50);
        pedal(&mut m, 0.05, Pedal::Sostenuto, 1.0);
        pedal(&mut m, 0.1, Pedal::Sustain, 0.0);
        let s = tocar(&mut m, 1.3);
        assert!(rms(&s, 1.1, 1.2) > rms(&s, 0.05, 0.1) * 0.05);
    }

    #[test]
    fn una_corda_mais_fraca_e_mais_escura() {
        let tocar_acorde = |una: f32| {
            let mut m = motor();
            pedal(&mut m, 0.0, Pedal::UnaCorda, una);
            for k in [27, 34, 43, 51] {
                on(&mut m, 0.01, k);
            }
            tocar(&mut m, 0.5)
        };
        let (normal, una) = (tocar_acorde(0.0), tocar_acorde(1.0));
        let db = 20.0 * (rms(&una, 0.0, 0.5) / rms(&normal, 0.0, 0.5)).log10();
        // brilho: energia da derivada (agudos) relativa ao sinal
        let brilho = |s: &[f32]| {
            let d: Vec<f32> = s.windows(3).step_by(2).map(|w| w[2] - w[0]).collect();
            (d.iter().map(|x| x * x).sum::<f32>() / s.iter().map(|x| x * x).sum::<f32>()).sqrt()
        };
        assert!((-6.0..-1.5).contains(&db), "una corda {db:.1} dB");
        assert!(brilho(&una) < brilho(&normal) * 0.95, "una corda deveria ser mais escura");
    }

    #[test]
    fn una_corda_no_guia_de_onda() {
        let tocar_acorde = |una: f32| {
            let mut m = Motor::new(Synth::new(TAXA, 1.0, None).com_modelo(Modelo::Guia), TAXA);
            pedal(&mut m, 0.0, Pedal::UnaCorda, una);
            for k in [27, 34, 43, 51] {
                on(&mut m, 0.01, k);
            }
            tocar(&mut m, 0.5)
        };
        let (normal, una) = (tocar_acorde(0.0), tocar_acorde(1.0));
        let db = 20.0 * (rms(&una, 0.0, 0.5) / rms(&normal, 0.0, 0.5)).log10();
        assert!((-8.0..-1.0).contains(&db), "una corda {db:.1} dB");
    }

    #[test]
    fn soltar_tudo_libera_teclas_e_pedais() {
        let mut m = motor();
        pedal(&mut m, 0.0, Pedal::Sustain, 1.0);
        on(&mut m, 0.0, 30);
        m.agendar(EventoAgendado { quadro: 4800, evento: Evento::SoltarTudo });
        let s = tocar(&mut m, 1.0);
        assert!(rms(&s, 0.8, 0.9) < rms(&s, 0.05, 0.1) * 1e-3);
        assert_eq!(m.synth().pedal(Pedal::Sustain), 0.0);
    }
}
