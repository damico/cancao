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

//! piano: teclado do terminal -> motor -> driver de áudio.
//!
//! A thread do driver só sintetiza; o teclado roda na thread principal e
//! manda eventos por uma fila sem lock (rtrb), para nunca travar o áudio.

use std::path::PathBuf;
use std::process::ExitCode;

use cancao::corda::{self, NUM_TECLAS};
use cancao::eventos::{Evento, EventoAgendado};
use cancao::motor::Motor;
use cancao::roteiro::{self, Roteiro};
use cancao::synth::{BLOCO, MAX_VOZES, Modelo, Synth};
use cancao::tampo::Tampo;
use cancao::wav;

const TAXA: u32 = 48000;

#[derive(PartialEq)]
enum Modo {
    Interativo,
    Tabela,
    /// toca um roteiro no dispositivo
    Tocar,
    /// grava um roteiro em WAV
    Render,
    Bench,
    Listar,
}

#[derive(Clone, Copy, PartialEq)]
enum QualRoteiro {
    Teclas,
    Demo,
    Pedais,
    Notas,
}

struct Opcoes {
    modo: Modo,
    roteiro: Option<QualRoteiro>,
    modelo: Modelo,
    wav: PathBuf,
    ir_entrada: Option<PathBuf>,
    ir_saida: Option<PathBuf>,
    sem_tampo: bool,
    notas: Vec<usize>,
    vel: f32,
    tensao: f64,
    dispositivo: String,
    /// 10 ms trava o driver SOF do notebook de desenvolvimento (ciclo de
    /// 64 amostras no PipeWire); ver c_sandbox/README.md
    latencia_us: u32,
    /// terminais sem aviso de tecla solta: solta a nota após este tempo
    segurar_s: f64,
}

const USO: &str = "\
uso: piano [opções]
  (sem opções)          modo interativo pelo teclado
  --tabela              mostra L, T, mu e f das 88 cordas
  --nota A4 [C#5 ...]   toca notas no dispositivo
  --demo                toca as 88 teclas em sequência
  --pedais              demonstração dos pedais sustain, una corda e sostenuto
  --render saida.wav    grava num WAV em vez de tocar (padrão: as 88 teclas;
                        combina com --nota, --demo e --pedais)
  --bench               mede o tempo de síntese no pior caso (32 vozes)
  --listar              lista os dispositivos de saída
  --modelo modal|guia   simulação da corda: soma de senoides (modal) ou
                        guia de onda digital
  --sem-tampo           só o som das cordas
  --tampo ir.wav        usa uma resposta ao impulso gravada
  --salvar-tampo x.wav  grava a resposta ao impulso usada
  --vel 0.7             força das notas (0..1)
  --tensao 1.0          multiplica a tensão de todas as cordas
  --dispositivo default dispositivo de saída
  --latencia 30000      buffer do driver em microssegundos
  --segurar 0.5         segundos até soltar a nota em terminais que não
                        avisam quando a tecla é solta";

fn ler_opcoes() -> Result<Opcoes, String> {
    let mut op = Opcoes {
        modo: Modo::Interativo,
        roteiro: None,
        modelo: Modelo::Modal,
        wav: PathBuf::new(),
        ir_entrada: None,
        ir_saida: None,
        sem_tampo: false,
        notas: Vec::new(),
        vel: 0.7,
        tensao: 1.0,
        dispositivo: "default".into(),
        latencia_us: 30000,
        segurar_s: 0.5,
    };
    let mut render = false;
    let mut args = std::env::args().skip(1).peekable();
    while let Some(a) = args.next() {
        let mut valor = || args.next().ok_or(format!("{a} precisa de um valor"));
        match a.as_str() {
            "--tabela" => op.modo = Modo::Tabela,
            "--demo" => op.roteiro = Some(QualRoteiro::Demo),
            "--pedais" => op.roteiro = Some(QualRoteiro::Pedais),
            "--bench" => op.modo = Modo::Bench,
            "--listar" => op.modo = Modo::Listar,
            "--render" => {
                render = true;
                op.wav = valor()?.into();
            }
            "--nota" => {
                op.roteiro = Some(QualRoteiro::Notas);
                while let Some(n) = args.next_if(|n| !n.starts_with('-')) {
                    op.notas.push(corda::tecla_de_nome(&n).ok_or(format!("nota inválida: {n}"))?);
                }
                if op.notas.is_empty() {
                    return Err("--nota precisa de pelo menos uma nota".into());
                }
            }
            "--sem-tampo" => op.sem_tampo = true,
            "--modelo" => {
                op.modelo = match valor()?.as_str() {
                    "modal" => Modelo::Modal,
                    "guia" => Modelo::Guia,
                    m => return Err(format!("modelo desconhecido: {m} (use modal ou guia)")),
                }
            }
            "--tampo" => op.ir_entrada = Some(valor()?.into()),
            "--salvar-tampo" => op.ir_saida = Some(valor()?.into()),
            "--vel" => op.vel = valor()?.parse().map_err(|_| "--vel inválido")?,
            "--tensao" => op.tensao = valor()?.parse().map_err(|_| "--tensao inválido")?,
            "--dispositivo" => op.dispositivo = valor()?,
            "--latencia" => op.latencia_us = valor()?.parse().map_err(|_| "--latencia inválido")?,
            "--segurar" => op.segurar_s = valor()?.parse().map_err(|_| "--segurar inválido")?,
            "-h" | "--ajuda" => {
                println!("{USO}");
                std::process::exit(0);
            }
            _ => return Err(format!("opção desconhecida: {a}\n{USO}")),
        }
    }
    if render {
        op.modo = Modo::Render;
        op.roteiro.get_or_insert(QualRoteiro::Teclas);
    } else if op.roteiro.is_some() && op.modo == Modo::Interativo {
        op.modo = Modo::Tocar;
    }
    Ok(op)
}

fn fazer_roteiro(op: &Opcoes) -> Roteiro {
    let mut r = match op.roteiro.unwrap_or(QualRoteiro::Teclas) {
        QualRoteiro::Teclas => roteiro::teclas(TAXA),
        QualRoteiro::Pedais => roteiro::pedais(TAXA),
        QualRoteiro::Notas => roteiro::notas(TAXA, &op.notas, op.vel, 0.6, None, 3.0),
        QualRoteiro::Demo => {
            let todas: Vec<usize> = (0..NUM_TECLAS).collect();
            roteiro::notas(TAXA, &todas, op.vel, 0.18, Some(0.5), 2.0)
        }
    };
    // sem textos próprios: mostra cada nota tocada
    if r.marcos.is_empty() {
        let cordas = corda::cordas(op.tensao);
        r.marcos = r
            .eventos
            .iter()
            .filter_map(|e| match e.evento {
                Evento::NotaOn { tecla, .. } => {
                    let c = &cordas[tecla as usize];
                    Some((e.quadro, format!("{}  {:.2} Hz", c.nome, c.f)))
                }
                _ => None,
            })
            .collect();
    }
    r
}

fn fazer_tampo(op: &Opcoes) -> Result<Option<Tampo>, String> {
    if op.sem_tampo {
        return Ok(None);
    }
    let t = match &op.ir_entrada {
        Some(p) => Tampo::de_wav(p, TAXA as f32, 3.0).map_err(|e| format!("{}: {e}", p.display()))?,
        None => Tampo::sintetico(TAXA as f32, 1.0, 12345),
    };
    Ok(Some(t))
}

fn fazer_motor(op: &Opcoes) -> Result<Motor, String> {
    let synth = Synth::new(TAXA as f32, op.tensao, fazer_tampo(op)?).com_modelo(op.modelo);
    Ok(Motor::new(synth, TAXA as f32))
}

fn imprimir_tabela(op: &Opcoes) {
    println!(
        "tecla nota   f_alvo(Hz)   L(m)   mu(g/m)    T(N)  f=1/2L*sqrt(T/mu)  \
         desvio(cents)        B   f1 real   cordas"
    );
    for c in corda::cordas(op.tensao) {
        println!(
            "{:4}  {:<4} {:10.3}  {:6.3}  {:8.3}  {:7.1}  {:14.3}  {:+12.2}  {:9.6}  {:8.3}   {}{}",
            c.tecla + 1,
            c.nome,
            c.f_alvo,
            c.l,
            c.mu * 1000.0,
            c.t,
            c.f,
            1200.0 * (c.f / c.f_alvo).log2(),
            c.b,
            c.parcial(1),
            c.n_cordas,
            if c.enrolada { " enrolada" } else { "" }
        );
    }
}

fn render_wav(op: &Opcoes) -> Result<(), String> {
    let mut m = fazer_motor(op)?;
    let tem_tampo = m.synth().tem_tampo();
    let r = fazer_roteiro(op);
    for e in r.a_partir_de(0) {
        m.agendar(e);
    }
    let mut audio = vec![0f32; 2 * r.duracao as usize];
    for parte in audio.chunks_mut(2 * BLOCO) {
        m.processar(parte);
    }
    wav::escrever_i16(&op.wav, TAXA, &audio).map_err(|e| format!("{}: {e}", op.wav.display()))?;
    println!(
        "gravado {} ({:.1} s, modelo: {}, tampo: {})",
        op.wav.display(),
        r.duracao as f64 / TAXA as f64,
        op.modelo.nome(),
        if tem_tampo { "sim" } else { "não" }
    );
    for (q, texto) in r.marcos.iter().filter(|_| op.roteiro == Some(QualRoteiro::Pedais)) {
        println!("  {:5.1} s  {texto}", *q as f64 / TAXA as f64);
    }
    println!(
        "tempo por bloco: médio {:.1} us, pico {:.1} us (prazo {:.0} us)",
        m.total.media_us(),
        m.total.pico_us,
        m.prazo_us
    );
    Ok(())
}

fn bench_um(rotulo: &str, synth: Synth, notas: bool) {
    let mut m = Motor::new(synth, TAXA as f32);
    // 32 notas na região mais cara (3 cordas, muitos parciais): G2..D5
    if notas {
        let g2 = corda::tecla_de_nome("G2").unwrap();
        for i in 0..MAX_VOZES {
            m.agendar(EventoAgendado::agora(Evento::NotaOn { tecla: (g2 + i) as u8, vel: 1.0 }));
        }
    }
    let mut buf = [0f32; 2 * BLOCO];
    for _ in 0..(2 * TAXA as usize / BLOCO) {
        m.processar(&mut buf);
    }
    let t = m.total;
    let pct = |us: f64| 100.0 * us / m.prazo_us;
    print!("  {rotulo:<28} médio {:7.1} us ({:5.1}%)", t.media_us(), pct(t.media_us()));
    if notas {
        print!(
            "   pico {:7.1} us ({:5.1}%)   blocos >50% do prazo: {}/{}",
            t.pico_us,
            pct(t.pico_us),
            t.acima_metade,
            t.n
        );
    }
    println!();
}

fn benchmark(op: &Opcoes) -> Result<(), String> {
    let prazo = 1e6 * BLOCO as f64 / TAXA as f64;
    println!("prazo por bloco: {BLOCO} quadros a {TAXA} Hz = {prazo:.0} us");
    println!("32 vozes simultâneas, força máxima, 2 s:");
    let taxa = TAXA as f32;
    let op_com_tampo = Opcoes { sem_tampo: false, ..clonar(op) };
    for modelo in [Modelo::Modal, Modelo::Guia] {
        let nome = modelo.nome();
        bench_um(&format!("{nome}: só cordas"), Synth::new(taxa, op.tensao, None).com_modelo(modelo), true);
        let synth = Synth::new(taxa, op.tensao, fazer_tampo(&op_com_tampo)?).com_modelo(modelo);
        bench_um(&format!("{nome}: cordas + tampo"), synth, true);
    }
    bench_um("só tampo (silêncio)", Synth::new(taxa, op.tensao, fazer_tampo(&op_com_tampo)?), false);
    println!("regra: até ~50% do prazo no pico, uma thread de áudio basta.");
    Ok(())
}

fn clonar(op: &Opcoes) -> Opcoes {
    Opcoes {
        modo: Modo::Bench,
        roteiro: op.roteiro,
        modelo: op.modelo,
        segurar_s: op.segurar_s,
        wav: op.wav.clone(),
        ir_entrada: op.ir_entrada.clone(),
        ir_saida: None,
        sem_tampo: op.sem_tampo,
        notas: op.notas.clone(),
        vel: op.vel,
        tensao: op.tensao,
        dispositivo: op.dispositivo.clone(),
        latencia_us: op.latencia_us,
    }
}

fn main() -> ExitCode {
    let op = match ler_opcoes() {
        Ok(op) => op,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(p) = &op.ir_saida {
        match fazer_tampo(&Opcoes { sem_tampo: false, ..clonar(&op) }) {
            Ok(Some(t)) => match t.salvar_ir(p, TAXA) {
                Ok(()) => println!("resposta ao impulso gravada em {}", p.display()),
                Err(e) => eprintln!("{}: {e}", p.display()),
            },
            Ok(None) => {}
            Err(e) => eprintln!("{e}"),
        }
    }

    let r = match op.modo {
        Modo::Tabela => {
            imprimir_tabela(&op);
            Ok(())
        }
        Modo::Render => render_wav(&op),
        Modo::Bench => benchmark(&op),
        _ => tempo_real::rodar(&op),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(feature = "audio"))]
mod tempo_real {
    pub fn rodar(_: &super::Opcoes) -> Result<(), String> {
        Err("compilado sem áudio (feature \"audio\"); só --tabela, --render e --bench".into())
    }
}

#[cfg(feature = "audio")]
mod tempo_real {
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::Ordering::Relaxed;
    use std::thread;
    use std::time::{Duration, Instant};

    use crossterm::event::{
        self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    };
    use crossterm::{cursor, execute, terminal};

    use super::*;
    use cancao::audio;
    use cancao::eventos::Pedal;
    use cancao::motor::Estado;

    pub fn rodar(op: &Opcoes) -> Result<(), String> {
        match op.modo {
            Modo::Listar => {
                for d in audio::listar() {
                    println!("{d}");
                }
                Ok(())
            }
            Modo::Tocar => tocar(op, &fazer_roteiro(op)),
            _ => interativo(op),
        }
    }

    fn restaurar_terminal() {
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags, cursor::Show);
        let _ = terminal::disable_raw_mode();
    }

    /// Se o driver/servidor de som parar de pedir amostras, o áudio fica
    /// mudo para sempre. O vigia observa o pulso do motor; parado por 3 s
    /// (inclusive na abertura do dispositivo), sai explicando.
    fn iniciar_vigia(estado: Arc<Estado>) {
        thread::spawn(move || {
            let (mut ultimo, mut desde) = (u64::MAX, Instant::now());
            loop {
                thread::sleep(Duration::from_millis(200));
                let p = estado.pulso.load(Relaxed);
                if p != ultimo {
                    (ultimo, desde) = (p, Instant::now());
                } else if desde.elapsed() > Duration::from_secs(3) {
                    restaurar_terminal();
                    eprintln!(
                        "\n\nO áudio parou de responder há 3 s: o servidor de som não está\n\
                         pedindo amostras (o piano não travou, o dispositivo sim).\n\
                         Teste outro programa:  pw-play /usr/share/sounds/alsa/Front_Center.wav\n\
                         Reiniciar o áudio:     systemctl --user restart pipewire pipewire-pulse wireplumber"
                    );
                    std::process::exit(2);
                }
            }
        });
    }

    fn abrir(op: &Opcoes) -> Result<(audio::Saida, rtrb::Producer<EventoAgendado>, Arc<Estado>), String> {
        let motor = fazer_motor(op)?;
        let estado = motor.estado();
        let (prod, cons) = rtrb::RingBuffer::new(4096);
        iniciar_vigia(estado.clone());
        let saida = audio::abrir(&op.dispositivo, TAXA, op.latencia_us, motor, cons)?;
        Ok((saida, prod, estado))
    }

    /// toca um roteiro no dispositivo e sai
    fn tocar(op: &Opcoes, r: &Roteiro) -> Result<(), String> {
        let (_saida, mut fila, estado) = abrir(op)?;
        // começa 50 ms à frente para o primeiro evento não chegar atrasado
        let inicio = estado.relogio.load(Relaxed) + TAXA as u64 / 20;
        for e in r.a_partir_de(inicio) {
            if fila.push(e).is_err() {
                return Err("roteiro maior que a fila de eventos".into());
            }
        }
        let mut marcos = r.marcos.iter().peekable();
        let mut linha_aberta = false;
        loop {
            let agora = estado.relogio.load(Relaxed);
            if agora >= inicio + r.duracao {
                break;
            }
            while let Some((_, texto)) = marcos.next_if(|(q, _)| inicio + q <= agora) {
                // marcos de nota sobrescrevem a linha; os de texto ficam
                if op.roteiro == Some(QualRoteiro::Pedais) {
                    println!("{texto}");
                } else {
                    print!("\r\x1b[K{texto}");
                    linha_aberta = true;
                }
                let _ = std::io::stdout().flush();
            }
            thread::sleep(Duration::from_millis(10));
        }
        if linha_aberta {
            println!();
        }
        Ok(())
    }

    /// duas fileiras como num tracker: z s x d c v g b h n j m , l . ; /
    /// (oitava base) e q 2 w 3 e r 5 t 6 y 7 u i 9 o 0 p (oitava acima)
    fn semitom_da_tecla(ch: char) -> Option<(i32, i32)> {
        const BAIXO: &str = "zsxdcvgbhnjm,l.;/";
        const CIMA: &str = "q2w3er5t6y7ui9o0p";
        if let Some(i) = BAIXO.find(ch) {
            return Some((i as i32, 0));
        }
        CIMA.find(ch).map(|i| (i as i32, 1))
    }

    fn pedal_da_tecla(code: KeyCode) -> Option<Pedal> {
        match code {
            KeyCode::Char(' ') => Some(Pedal::Sustain),
            KeyCode::Char('1') => Some(Pedal::UnaCorda),
            KeyCode::Char('4') => Some(Pedal::Sostenuto),
            _ => None,
        }
    }

    fn interativo(op: &Opcoes) -> Result<(), String> {
        let cordas = corda::cordas(op.tensao);
        let (saida, mut fila, estado) = abrir(op)?;
        let prazo_us = 1e6 * BLOCO as f64 / TAXA as f64;

        terminal::enable_raw_mode().map_err(|e| e.to_string())?;
        // protocolo de teclado do kitty: o terminal avisa quando a tecla é
        // solta (kitty, WezTerm, foot, Ghostty...); sem ele, só há "pressionou"
        let solta_real = terminal::supports_keyboard_enhancement().unwrap_or(false);
        if solta_real {
            let _ = execute!(
                std::io::stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
            );
        }
        let _ = execute!(std::io::stdout(), cursor::Hide);
        let gancho = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restaurar_terminal();
            gancho(info);
        }));

        let pedais_ajuda = if solta_real {
            "pedais (segure): espaço sustain, 1 una corda, 4 sostenuto"
        } else {
            "pedais (liga/desliga): espaço sustain, 1 una corda, 4 sostenuto"
        };
        let teclas_ajuda = if solta_real {
            "segure a tecla para segurar a nota".to_string()
        } else {
            format!("este terminal não avisa quando a tecla é solta: nota solta após {} s (--segurar)", op.segurar_s)
        };
        print!(
            "Piano digital - 88 teclas (A0..C8)   [{}]\r\n\
             \x20 z s x d c v g b h n j m , l . ; /   -> oitava base\r\n\
             \x20 q 2 w 3 e r 5 t 6 y 7 u i 9 o 0 p   -> oitava de cima\r\n\
             \x20 {pedais_ajuda}\r\n\
             \x20 - / =  oitava   [ / ]  força   Backspace  soltar tudo   Tab  tampo   8  modelo   Esc  sair\r\n\
             \x20 {teclas_ajuda}\r\n\
             \x20 CPU = tempo de síntese / prazo de {prazo_us:.0} us por bloco (média | pico em 0,5 s)\r\n\r\n",
            saida.descricao
        );

        let (mut oitava, mut vel) = (3i32, 0.7f32);
        let mut ultima = String::new();
        // tecla do teclado do computador -> tecla do piano, enquanto abaixada
        let mut abaixadas: [Option<u8>; 128] = [None; 128];
        // modo sem aviso de soltura: quando soltar cada nota
        let mut soltar_em: [Option<Instant>; NUM_TECLAS] = [None; NUM_TECLAS];
        let segurar = Duration::from_secs_f64(op.segurar_s);
        let mut enviar = |ev| {
            let _ = fila.push(EventoAgendado::agora(ev));
        };

        loop {
            if event::poll(Duration::from_millis(20)).unwrap_or(false) {
                while event::poll(Duration::ZERO).unwrap_or(false) {
                    let Ok(Event::Key(k)) = event::read() else { continue };
                    let solta = k.kind == KeyEventKind::Release;
                    if k.kind == KeyEventKind::Repeat {
                        continue;
                    }

                    if let Some(p) = pedal_da_tecla(k.code) {
                        let valor = if solta_real {
                            if solta { 0.0 } else { 1.0 }
                        } else if estado.pedal(p) > 0.0 {
                            0.0
                        } else {
                            1.0
                        };
                        enviar(Evento::Pedal { pedal: p, valor });
                        continue;
                    }

                    if let KeyCode::Char(ch) = k.code {
                        if let Some((st, extra)) = semitom_da_tecla(ch) {
                            let i = ch as usize % 128;
                            if solta {
                                if let Some(tecla) = abaixadas[i].take() {
                                    enviar(Evento::NotaOff { tecla });
                                }
                                continue;
                            }
                            let t = (oitava + extra + 1) * 12 + st - 21;
                            if !(0..NUM_TECLAS as i32).contains(&t) {
                                continue;
                            }
                            let tecla = t as u8;
                            abaixadas[i] = Some(tecla);
                            enviar(Evento::NotaOn { tecla, vel });
                            if !solta_real {
                                soltar_em[t as usize] = Some(Instant::now() + segurar);
                            }
                            let c = &cordas[t as usize];
                            ultima = format!(
                                "{:<4} f={:.2} Hz  L={:.3} m  T={:.0} N  mu={:.2} g/m  B={:.5}",
                                c.nome,
                                c.f,
                                c.l,
                                c.t,
                                c.mu * 1000.0,
                                c.b
                            );
                            continue;
                        }
                    }

                    if solta {
                        continue;
                    }
                    match k.code {
                        KeyCode::Esc => {
                            restaurar_terminal();
                            println!();
                            return Ok(());
                        }
                        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                            restaurar_terminal();
                            println!();
                            return Ok(());
                        }
                        KeyCode::Tab => enviar(Evento::AlternarTampo),
                        KeyCode::Char('8') => enviar(Evento::AlternarModelo),
                        KeyCode::Backspace => {
                            enviar(Evento::SoltarTudo);
                            abaixadas = [None; 128];
                            soltar_em = [None; NUM_TECLAS];
                        }
                        KeyCode::Char('-') => oitava = (oitava - 1).max(0),
                        KeyCode::Char('=') => oitava = (oitava + 1).min(7),
                        KeyCode::Char('[') => vel = (vel - 0.1).max(0.1),
                        KeyCode::Char(']') => vel = (vel + 0.1).min(1.0),
                        _ => {}
                    }
                }
            }

            let agora = Instant::now();
            for (t, s) in soltar_em.iter_mut().enumerate() {
                if s.is_some_and(|s| s <= agora) {
                    *s = None;
                    enviar(Evento::NotaOff { tecla: t as u8 });
                }
            }

            let marca = |p| {
                let v = estado.pedal(p);
                if v >= 0.99 { "▼".to_string() } else if v > 0.0 { format!("{:.0}%", 100.0 * v) } else { "-".to_string() }
            };
            print!(
                "\r\x1b[KC{oitava}  força {vel:.1}  {}  vozes {:2}/{MAX_VOZES}  sust {} una {} sost {}  tampo {}  CPU {:4.1}% | {:4.1}%{}  | {ultima}",
                if estado.modelo_guia.load(Relaxed) { "guia" } else { "modal" },
                estado.vozes.load(Relaxed),
                marca(Pedal::Sustain),
                marca(Pedal::UnaCorda),
                marca(Pedal::Sostenuto),
                if estado.tampo_ligado.load(Relaxed) { "sim" } else { "não" },
                estado.carga_media.load(Relaxed) as f64 / 100.0,
                estado.carga_pico.load(Relaxed) as f64 / 100.0,
                if estado.tempo_real.load(Relaxed) { "  RT" } else { "" },
            );
            let _ = std::io::stdout().flush();
        }
    }
}
