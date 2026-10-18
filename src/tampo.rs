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

//! Tampo harmônico + sala por convolução.
//!
//! As cordas quase não movem ar: quem irradia o som é o tampo, excitado
//! pelo cavalete. Modelamos isso como um sistema linear: a soma das cordas
//! (mono, no cavalete) é convoluída com a resposta ao impulso do tampo +
//! sala, uma para cada ouvido (estéreo).
//!
//! Convolução particionada uniforme (overlap-save) com blocos de [`BLOCO`]
//! amostras: custo constante por bloco e nenhuma latência além do próprio
//! bloco de áudio.
//!
//! Truque usado: as respostas esquerda (hL) e direita (hR) são reais, assim
//! como a entrada x. Guardando H = FFT(hL + j*hR), uma única convolução
//! complexa dá y = x*hL + j*(x*hR): parte real = canal esquerdo, parte
//! imaginária = canal direito.

use std::f32::consts::PI;
use std::io;
use std::path::Path;

use crate::wav;

pub const BLOCO: usize = 128;
const N_FFT: usize = 2 * BLOCO;

pub struct Tampo {
    n_part: usize,
    /// espectros da resposta: n_part × N_FFT
    h_re: Vec<f32>,
    h_im: Vec<f32>,
    /// espectros das últimas entradas (anel)
    x_re: Vec<f32>,
    x_im: Vec<f32>,
    /// partição mais recente no anel
    pos: usize,
    /// bloco anterior (overlap-save)
    anterior: [f32; BLOCO],
    tw_re: [f32; N_FFT / 2],
    tw_im: [f32; N_FFT / 2],
    rev: [usize; N_FFT],
    /// resposta no tempo, para salvar
    ir_l: Vec<f32>,
    ir_r: Vec<f32>,
}

impl Tampo {
    fn de_ir(ir_l: Vec<f32>, ir_r: Vec<f32>) -> Self {
        let len = ir_l.len();
        let n_part = len.div_ceil(BLOCO);
        let bits = N_FFT.trailing_zeros();
        let mut t = Tampo {
            n_part,
            h_re: vec![0.0; n_part * N_FFT],
            h_im: vec![0.0; n_part * N_FFT],
            x_re: vec![0.0; n_part * N_FFT],
            x_im: vec![0.0; n_part * N_FFT],
            pos: 0,
            anterior: [0.0; BLOCO],
            tw_re: std::array::from_fn(|i| (-2.0 * PI * i as f32 / N_FFT as f32).cos()),
            tw_im: std::array::from_fn(|i| (-2.0 * PI * i as f32 / N_FFT as f32).sin()),
            rev: std::array::from_fn(|i| i.reverse_bits() >> (usize::BITS - bits)),
            ir_l,
            ir_r,
        };

        // cada partição: BLOCO amostras da resposta + zeros
        for p in 0..n_part {
            let faixa = p * N_FFT..(p + 1) * N_FFT;
            let (mut re, mut im) = ([0f32; N_FFT], [0f32; N_FFT]);
            for i in 0..BLOCO {
                let k = p * BLOCO + i;
                if k < len {
                    re[i] = t.ir_l[k];
                    im[i] = t.ir_r[k];
                }
            }
            t.fft(&mut re, &mut im, false);
            t.h_re[faixa.clone()].copy_from_slice(&re);
            t.h_im[faixa].copy_from_slice(&im);
        }
        t
    }

    /// Resposta ao impulso sintética (modos de placa + cauda de sala).
    pub fn sintetico(taxa: f32, duracao_s: f32, semente: u32) -> Self {
        let len = (duracao_s * taxa) as usize;
        let mut h = [vec![0f32; len], vec![0f32; len]];
        let mut s = semente;

        // tampo: ~300 modos de placa de 50 Hz a 9 kHz, cada um com Q entre
        // 15 e 50; a madeira amortece muito os agudos, por isso o decaimento
        // (tau = Q / (π f)) é curto em frequências altas. Os dois ouvidos
        // escutam os mesmos modos (é a mesma placa), com amplitude e fase
        // um pouco diferentes conforme a posição
        for _ in 0..300 {
            let f = 50.0 * (9000.0f32 / 50.0).powf(aleatorio(&mut s));
            let q = 15.0 + 35.0 * aleatorio(&mut s);
            let tau = q / (PI * f);
            let fase = 2.0 * PI * aleatorio(&mut s);
            // resposta média do tampo: sobe até ~150 Hz, cai acima de ~2,5 kHz
            let forma = (f / (f + 150.0)) / (1.0 + (f / 2500.0) * (f / 2500.0));
            let base = forma * (0.3 + aleatorio(&mut s));
            let amp = if aleatorio(&mut s) < 0.5 { -base } else { base };

            let n_max = ((tau * 7.0 * taxa) as usize).min(len); // até ~-60 dB
            let w = 2.0 * PI * f / taxa;
            let dec = (-1.0 / (tau * taxa)).exp();
            for canal in h.iter_mut() {
                let mut a = amp * (0.6 + 0.8 * aleatorio(&mut s));
                let ph = fase + 0.8 * (aleatorio(&mut s) - 0.5);
                // oscilador recursivo: evita sin() por amostra
                let (mut c, mut sn) = (ph.cos(), ph.sin());
                let (cr, sr) = (w.cos(), w.sin());
                for x in &mut canal[..n_max] {
                    *x += a * sn;
                    let nc = c * cr - sn * sr;
                    sn = c * sr + sn * cr;
                    c = nc;
                    a *= dec;
                }
            }
        }

        for canal in h.iter_mut() {
            // sala: ruído com decaimento exponencial (RT60 ~1 s), começando
            // após ~8 ms (primeiras reflexões), filtrado para escurecer
            let rt60 = 1.0f32;
            let tau_sala = rt60 / 6.91;
            let ini = (0.008 * taxa) as usize;
            let (mut lp, nivel) = (0f32, 0.05f32);
            for i in ini..len {
                let t_s = (i - ini) as f32 / taxa;
                let ataque = if t_s < 0.02 { t_s / 0.02 } else { 1.0 };
                let r = 2.0 * aleatorio(&mut s) - 1.0;
                lp += 0.35 * (r - lp);
                canal[i] += nivel * lp * ataque * (-t_s / tau_sala).exp();
            }
            normalizar(canal);
        }

        let [l, r] = h;
        Self::de_ir(l, r)
    }

    /// Resposta ao impulso de um WAV (PCM 16 bits ou float 32, mono ou estéreo).
    pub fn de_wav(caminho: &Path, taxa: f32, duracao_max_s: f32) -> io::Result<Self> {
        let w = wav::ler(caminho)?;
        if w.taxa != taxa as u32 {
            eprintln!(
                "aviso: {} está a {} Hz, esperado {} Hz (sem reamostragem)",
                caminho.display(),
                w.taxa,
                taxa
            );
        }
        let max = (duracao_max_s * taxa) as usize;
        let mut l: Vec<f32> = w.canais[0].iter().copied().take(max).collect();
        let mut r: Vec<f32> = w.canais[w.canais.len().min(2) - 1].iter().copied().take(max).collect();
        if l.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "resposta ao impulso vazia"));
        }
        normalizar(&mut l);
        normalizar(&mut r);
        Ok(Self::de_ir(l, r))
    }

    /// Grava a resposta ao impulso em WAV estéreo float (para ouvir/inspecionar),
    /// com o pico normalizado para ouvir confortavelmente.
    pub fn salvar_ir(&self, caminho: &Path, taxa: u32) -> io::Result<()> {
        let pico = self.ir_l.iter().chain(&self.ir_r).fold(1e-9f32, |m, x| m.max(x.abs()));
        let quadros: Vec<f32> = self
            .ir_l
            .iter()
            .zip(&self.ir_r)
            .flat_map(|(l, r)| [0.9 * l / pico, 0.9 * r / pico])
            .collect();
        wav::escrever_f32(caminho, taxa, &quadros)
    }

    /// in-place; `inversa` conjuga os fatores e divide por N
    fn fft(&self, re: &mut [f32; N_FFT], im: &mut [f32; N_FFT], inversa: bool) {
        for i in 0..N_FFT {
            let j = self.rev[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let sinal = if inversa { -1.0 } else { 1.0 };
        let mut tam = 2;
        while tam <= N_FFT {
            let (meio, passo) = (tam / 2, N_FFT / tam);
            for ini in (0..N_FFT).step_by(tam) {
                for k in 0..meio {
                    let (wr, wi) = (self.tw_re[k * passo], sinal * self.tw_im[k * passo]);
                    let (a, b) = (ini + k, ini + k + meio);
                    let br = re[b] * wr - im[b] * wi;
                    let bi = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - br;
                    im[b] = im[a] - bi;
                    re[a] += br;
                    im[a] += bi;
                }
            }
            tam <<= 1;
        }
        if inversa {
            for x in re.iter_mut().chain(im.iter_mut()) {
                *x /= N_FFT as f32;
            }
        }
    }

    /// Processa exatamente um bloco: entrada mono, saída estéreo intercalada.
    pub fn processar(&mut self, entrada: &[f32; BLOCO], saida: &mut [f32; 2 * BLOCO]) {
        // espectro de [bloco anterior | bloco atual] entra no anel
        self.pos = (self.pos + 1) % self.n_part;
        let (mut xr, mut xi) = ([0f32; N_FFT], [0f32; N_FFT]);
        xr[..BLOCO].copy_from_slice(&self.anterior);
        xr[BLOCO..].copy_from_slice(entrada);
        self.anterior = *entrada;
        self.fft(&mut xr, &mut xi, false);
        let faixa = self.pos * N_FFT..(self.pos + 1) * N_FFT;
        self.x_re[faixa.clone()].copy_from_slice(&xr);
        self.x_im[faixa].copy_from_slice(&xi);

        // Y = Σ_p X[atual − p] * H[p]
        let (mut yr, mut yi) = ([0f32; N_FFT], [0f32; N_FFT]);
        for p in 0..self.n_part {
            let q = (self.pos + self.n_part - p) % self.n_part;
            let ar = &self.x_re[q * N_FFT..(q + 1) * N_FFT];
            let ai = &self.x_im[q * N_FFT..(q + 1) * N_FFT];
            let hr = &self.h_re[p * N_FFT..(p + 1) * N_FFT];
            let hi = &self.h_im[p * N_FFT..(p + 1) * N_FFT];
            for k in 0..N_FFT {
                yr[k] += ar[k] * hr[k] - ai[k] * hi[k];
                yi[k] += ar[k] * hi[k] + ai[k] * hr[k];
            }
        }
        self.fft(&mut yr, &mut yi, true);

        // overlap-save: a metade final é a saída válida
        for i in 0..BLOCO {
            saida[2 * i] = yr[BLOCO + i];
            saida[2 * i + 1] = yi[BLOCO + i];
        }
    }
}

/// normaliza a energia para 1 (volume parecido com o som seco)
fn normalizar(h: &mut [f32]) {
    let e: f64 = h.iter().map(|&x| x as f64 * x as f64).sum();
    if e > 0.0 {
        let g = (1.0 / e.sqrt()) as f32;
        h.iter_mut().for_each(|x| *x *= g);
    }
}

/// uniforme em [0, 1)
fn aleatorio(s: &mut u32) -> f32 {
    *s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*s >> 8) as f32 * (1.0 / 16_777_216.0)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn convolucao_igual_a_direta() {
        let mut t = Tampo::sintetico(48000.0, 0.2, 7);
        let nb = 200;
        let mut s = 1u32;
        let x: Vec<f32> = (0..BLOCO * nb).map(|_| 2.0 * aleatorio(&mut s) - 1.0).collect();
        let mut y = vec![0f32; 2 * BLOCO * nb];
        for b in 0..nb {
            let ent: &[f32; BLOCO] = x[b * BLOCO..(b + 1) * BLOCO].try_into().unwrap();
            let sai: &mut [f32; 2 * BLOCO] = (&mut y[2 * b * BLOCO..2 * (b + 1) * BLOCO]).try_into().unwrap();
            t.processar(ent, sai);
        }
        let (mut erro, mut ref_max) = (0f64, 0f64);
        for n in (0..x.len()).step_by(37) {
            let (mut l, mut r) = (0f64, 0f64);
            for k in 0..t.ir_l.len().min(n + 1) {
                l += t.ir_l[k] as f64 * x[n - k] as f64;
                r += t.ir_r[k] as f64 * x[n - k] as f64;
            }
            erro = erro.max((l - y[2 * n] as f64).abs()).max((r - y[2 * n + 1] as f64).abs());
            ref_max = ref_max.max(l.abs());
        }
        assert!(erro < 1e-4 * ref_max, "erro {erro:e} (sinal ~{ref_max})");
    }
}
