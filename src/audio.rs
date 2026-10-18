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

//! Saída de áudio em tempo real com cpal (ALSA/PipeWire/JACK no Linux,
//! CoreAudio no macOS, WASAPI no Windows).
//!
//! O driver chama a função de áudio de uma thread própria. Ela só esvazia
//! a fila de eventos e chama o motor: sem alocação, sem locks.

use std::sync::atomic::Ordering::Relaxed;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::eventos::EventoAgendado;
use crate::motor::Motor;

/// Mantém o áudio tocando enquanto existir.
pub struct Saida {
    _stream: cpal::Stream,
    pub descricao: String,
}

pub fn listar() -> Vec<String> {
    let host = cpal::default_host();
    host.output_devices()
        .map(|ds| ds.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

fn escolher(nome: &str) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    if nome == "default" {
        return host.default_output_device().ok_or("sem dispositivo de saída padrão".into());
    }
    host.output_devices()
        .map_err(|e| e.to_string())?
        .find(|d| d.name().is_ok_and(|n| n == nome))
        .ok_or_else(|| format!("dispositivo '{nome}' não encontrado (veja --listar)"))
}

/// Prioridade de tempo real para a thread do driver: o escalonador não a
/// interrompe para rodar outros programas. Precisa de permissão (rtprio);
/// se falhar, segue com prioridade normal.
fn pedir_tempo_real() -> bool {
    let sp = libc::sched_param { sched_priority: 70 };
    unsafe { libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &sp) == 0 }
}

pub fn abrir(
    dispositivo: &str,
    taxa: u32,
    latencia_us: u32,
    motor: Motor,
    fila: rtrb::Consumer<EventoAgendado>,
) -> Result<Saida, String> {
    let dev = escolher(dispositivo)?;
    let formato = dev
        .supported_output_configs()
        .map_err(|e| e.to_string())?
        .filter(|c| c.channels() == 2 && (c.min_sample_rate().0..=c.max_sample_rate().0).contains(&taxa))
        .map(|c| c.sample_format())
        .min_by_key(|f| match f {
            SampleFormat::F32 => 0,
            SampleFormat::I16 => 1,
            SampleFormat::I32 => 2,
            _ => 3,
        })
        .ok_or_else(|| format!("o dispositivo não aceita estéreo a {taxa} Hz"))?;

    let quadros = (taxa as u64 * latencia_us as u64 / 1_000_000) as u32;
    let config = cpal::StreamConfig {
        channels: 2,
        sample_rate: cpal::SampleRate(taxa),
        buffer_size: cpal::BufferSize::Fixed(quadros),
    };
    let stream = match formato {
        SampleFormat::F32 => construir::<f32>(&dev, &config, motor, fila),
        SampleFormat::I16 => construir::<i16>(&dev, &config, motor, fila),
        SampleFormat::I32 => construir::<i32>(&dev, &config, motor, fila),
        f => return Err(format!("formato de amostra não suportado: {f}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    let nome = dev.name().unwrap_or_default();
    Ok(Saida {
        _stream: stream,
        descricao: format!("{nome}, {formato}, buffer {quadros} quadros"),
    })
}

fn construir<T>(
    dev: &cpal::Device,
    config: &cpal::StreamConfig,
    mut motor: Motor,
    mut fila: rtrb::Consumer<EventoAgendado>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let estado = motor.estado();
    let mut primeira = true;
    // alocado uma vez, fora da thread de áudio
    let mut tmp = vec![0f32; 16384];
    dev.build_output_stream(
        config,
        move |dados: &mut [T], _: &cpal::OutputCallbackInfo| {
            if primeira {
                estado.tempo_real.store(pedir_tempo_real(), Relaxed);
                primeira = false;
            }
            while let Ok(ev) = fila.pop() {
                motor.agendar(ev);
            }
            for parte in dados.chunks_mut(tmp.len()) {
                let buf = &mut tmp[..parte.len()];
                motor.processar(buf);
                for (d, &s) in parte.iter_mut().zip(buf.iter()) {
                    *d = T::from_sample(s);
                }
            }
            estado.pulso.fetch_add(1, Relaxed);
        },
        |e| eprintln!("\r\nerro de áudio: {e}\r"),
        None,
    )
    .map_err(|e| e.to_string())
}
