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

//! Leitura e escrita mínima de WAV (PCM 16 bits e float 32).

use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub struct Wav {
    pub taxa: u32,
    /// um vetor de amostras por canal
    pub canais: Vec<Vec<f32>>,
}

fn cabecalho(f: &mut impl Write, formato: u16, bits: u16, taxa: u32, n_quadros: usize) -> io::Result<()> {
    let bytes_quadro = 2 * bits as u32 / 8;
    let dados = n_quadros as u32 * bytes_quadro;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + dados).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&formato.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?; // estéreo
    f.write_all(&taxa.to_le_bytes())?;
    f.write_all(&(taxa * bytes_quadro).to_le_bytes())?;
    f.write_all(&(bytes_quadro as u16).to_le_bytes())?;
    f.write_all(&bits.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&dados.to_le_bytes())
}

/// Grava áudio estéreo intercalado como PCM 16 bits.
pub fn escrever_i16(caminho: &Path, taxa: u32, quadros: &[f32]) -> io::Result<()> {
    let mut f = io::BufWriter::new(fs::File::create(caminho)?);
    cabecalho(&mut f, 1, 16, taxa, quadros.len() / 2)?;
    for &x in quadros {
        f.write_all(&((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
    }
    f.flush()
}

/// Grava áudio estéreo intercalado como float 32.
pub fn escrever_f32(caminho: &Path, taxa: u32, quadros: &[f32]) -> io::Result<()> {
    let mut f = io::BufWriter::new(fs::File::create(caminho)?);
    cabecalho(&mut f, 3, 32, taxa, quadros.len() / 2)?;
    for &x in quadros {
        f.write_all(&x.to_le_bytes())?;
    }
    f.flush()
}

fn invalido(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Lê WAV PCM 16 bits ou float 32, com qualquer número de canais.
pub fn ler(caminho: &Path) -> io::Result<Wav> {
    let d = fs::read(caminho)?;
    if d.len() < 12 || &d[0..4] != b"RIFF" || &d[8..12] != b"WAVE" {
        return Err(invalido("não é WAV"));
    }
    let u16_em = |p: usize| u16::from_le_bytes([d[p], d[p + 1]]);
    let u32_em = |p: usize| u32::from_le_bytes([d[p], d[p + 1], d[p + 2], d[p + 3]]);

    let (mut formato, mut n_canais, mut taxa, mut bits) = (0, 0, 0, 0);
    let mut p = 12;
    while p + 8 <= d.len() {
        let n = u32_em(p + 4) as usize;
        let c = p + 8;
        match &d[p..p + 4] {
            b"fmt " if n >= 16 => {
                formato = u16_em(c);
                n_canais = u16_em(c + 2) as usize;
                taxa = u32_em(c + 4);
                bits = u16_em(c + 14);
            }
            b"data" => {
                let pcm16 = formato == 1 && bits == 16;
                let f32_ = formato == 3 && bits == 32;
                if !(pcm16 || f32_) || n_canais == 0 {
                    return Err(invalido("use PCM 16 bits ou float 32"));
                }
                let fim = (c + n).min(d.len());
                let tam = bits as usize / 8;
                let amostras = (fim - c) / tam;
                let mut canais = vec![Vec::with_capacity(amostras / n_canais); n_canais];
                for i in 0..amostras / n_canais * n_canais {
                    let q = c + i * tam;
                    let v = if pcm16 {
                        u16_em(q) as i16 as f32 / 32768.0
                    } else {
                        f32::from_bits(u32_em(q))
                    };
                    canais[i % n_canais].push(v);
                }
                return Ok(Wav { taxa, canais });
            }
            _ => {}
        }
        p += 8 + n + (n & 1);
    }
    Err(invalido("sem bloco de dados"))
}
