# cancao: piano digital por modelagem física (Rust)

Sintetiza as 88 teclas de um piano (A0-C8) a partir da física da corda:

    f = (1 / 2L) * √(T / μ)

Cada corda tem comprimento `L`, massa linear `μ` e diâmetro plausíveis para
um piano de cauda. A tensão `T` sai da afinação, como faz um afinador, e a
frequência que soa é calculada pela fórmula. A rigidez do aço dá a
inarmonicidade dos parciais: `f_n = n*f*√(1 + B*n²)`, com
`B = π³*E*d⁴ / (64*T*L²)`.

O protótipo original em C está em [`c_sandbox/`](c_sandbox/), incluindo a
investigação do travamento do driver de áudio do notebook
([Áudio: latência, driver e hardware](c_sandbox/README.md#áudio-latência-driver-e-hardware)).

## Plataformas

Foco em Linux (Ubuntu) e macOS; Windows em segundo plano. A saída de
áudio usa `cpal`, que cobre ALSA/PipeWire/JACK, CoreAudio e WASAPI. A
interface gráfica será feita em [Slint](https://slint.dev).

## Compilar

    sudo apt install libasound2-dev pkg-config   # só no Linux, uma vez
    cargo build --release
    cargo test --release

Sem os headers do ALSA, dá para compilar só a parte offline
(`--tabela`, `--render`, `--bench`):

    cargo build --release --no-default-features

## Usar

    ./target/release/piano                  # modo interativo
    ./target/release/piano --tabela         # L, μ, T, f e B das 88 cordas
    ./target/release/piano --nota A4 C#5    # toca notas e sai
    ./target/release/piano --demo           # as 88 teclas em sequência
    ./target/release/piano --pedais         # demonstração dos 3 pedais
    ./target/release/piano --render x.wav   # grava as 88 teclas num WAV
    ./target/release/piano --render x.wav --pedais   # grava a demonstração
    ./target/release/piano --bench          # tempo de síntese, pior caso
    ./target/release/piano --listar         # dispositivos de saída
    ./target/release/piano --modelo guia    # corda em guia de onda (padrão: modal)
    ./target/release/piano --sem-tampo      # só as cordas, sem tampo/sala
    ./target/release/piano --tampo ir.wav   # resposta ao impulso gravada
    ./target/release/piano --tensao 1.02    # cordas 2% mais tensas (aproximadamente +17 cents)

Teclado no modo interativo:

    z s x d c v g b h n j m , l . ; /    oitava base
    q 2 w 3 e r 5 t 6 y 7 u i 9 o 0 p    oitava de cima
    espaço  sustain      1  una corda      4  sostenuto
    - / =   oitava       [ / ]  força
    Backspace  solta todas as teclas e pedais
    Tab     liga/desliga o tampo           8    troca o modelo (modal/guia)
    Esc     sai

Terminais com o protocolo de teclado do kitty (kitty, WezTerm, foot,
Ghostty) avisam quando a tecla é solta: segurar a tecla segura a nota e
segurar o espaço segura o pedal. Nos outros (ex.: GNOME Terminal) a nota é
solta sozinha após 0,5 s (`--segurar`) e os pedais funcionam como
liga/desliga. O cabeçalho do modo interativo diz qual é o caso.

A linha de status mostra `CPU média | pico`: quanto do prazo de cada bloco
de áudio (128 quadros = 2,67 ms) a síntese consumiu no último meio segundo.
`RT` aparece quando a thread de áudio conseguiu prioridade de tempo real.

O buffer padrão é de 30 ms (`--latencia 30000`): com 10 ms o driver Intel
SOF do notebook de desenvolvimento trava. Se o áudio parar por 3 s, o
programa sai explicando como reiniciar o PipeWire.

## Modelos de corda

- **modal** (`src/modal.rs`): soma de senoides, um oscilador por parcial e
  por corda; imita o resultado da física (inarmonicidade, decaimento em dois
  estágios, posição do martelo).
- **guia de onda** (`src/guia.rs`): simula a onda viajando pela corda. Uma
  volta é uma linha de atraso de `taxa / f` amostras, com filtro de perdas
  (passa-baixa de um polo) e passa-tudo de atraso fracionário para afinação
  exata. O martelo injeta um pulso de força a 1/8 da corda, e a reflexão na
  ponta próxima apaga os harmônicos múltiplos de 8 sozinha. Custa cerca de
  1/3 do modal (`--bench`).

Etapas do guia de onda:

1. ~~Laço afinado por L, T e μ, perdas, martelo como pulso~~ (afinação
   medida nas 88 teclas: no máximo 1,2 cent)
2. Rigidez: filtros de dispersão (inarmonicidade B)
3. Martelo não linear (feltro como mola dura)
4. Cordas do uníssono acopladas no cavalete (decaimento em dois estágios,
   batimentos, ressonância simpática)

Nesta etapa o guia de onda ainda soa mais "harpa" que piano: os parciais
são harmônicos (sem B) e o decaimento é único.

## Pedais

| pedal | no piano real | no modelo |
|---|---|---|
| sustain (direito) | levanta todos os abafadores | contínuo 0..1; até 25% abafa inteiro, a partir de 75% levanta, entre os dois o abafador roça a corda (meio pedal) |
| una corda (esquerdo) | desloca a mecânica: o martelo acerta uma corda a menos, com feltro mais macio | uma corda a menos (aproximadamente −2 dB), timbre mais escuro, a corda não atingida vibra fraca por simpatia; vale para as notas tocadas depois |
| sostenuto (meio) | prende os abafadores levantados no instante em que desce | prende as teclas abaixadas naquele instante; com o sustain abaixado prende todas, como num piano de cauda |

Do F#6 para cima não há abafadores. Mensagens MIDI de nota e os controles
64 (sustain), 66 (sostenuto) e 67 (una corda) já têm tradução para os
eventos internos (`eventos::de_midi`), para quando houver entrada MIDI.

## Arquitetura

    quem toca (terminal, Roteiro; depois MIDI e o player de partituras)
        │  EventoAgendado { quadro, evento }  ── fila sem lock (rtrb)
        │  evento: NotaOn, NotaOff, Pedal, SoltarTudo, AlternarTampo
        ▼
    Motor  ── aplica eventos no instante certo, gera blocos, mede o tempo
        ▼
    Synth  ── vozes (síntese modal) -> cavalete -> Tampo (convolução) -> estéreo
        ▼
    driver (cpal: ALSA/PipeWire/JACK, CoreAudio, WASAPI)

| arquivo          | função                                                     |
|------------------|------------------------------------------------------------|
| `src/corda.rs`   | física: L, μ, T, f, B de cada tecla                        |
| `src/eventos.rs` | interface entre quem toca e o instrumento; tradução MIDI   |
| `src/roteiro.rs` | performance pronta (eventos com horário): demos e testes   |
| `src/motor.rs`   | agenda eventos por quadro, blocos de 128, medição de tempo |
| `src/synth.rs`   | instrumento: vozes, teclas, abafadores, pedais, cavalete e tampo |
| `src/modal.rs`   | corda por síntese modal (48 parciais × 3 cordas)            |
| `src/guia.rs`    | corda por guia de onda digital                              |
| `src/tampo.rs`   | tampo + sala: convolução particionada com FFT própria      |
| `src/audio.rs`   | saída em tempo real (cpal), prioridade RT                  |
| `src/wav.rs`     | leitura/escrita de WAV                                     |
| `src/main.rs`    | linha de comando, terminal, vigia do áudio                 |

O mesmo `Motor` é usado no tempo real, na gravação de WAV e no benchmark,
e nada nele aloca memória depois de criado. Os eventos carregam o quadro
exato em que devem acontecer, o que permite ao futuro player mandar uma
partitura inteira de uma vez. Hoje são aplicados no início do bloco que
contém o instante (resolução de 2,7 ms).

## Verificação do port de C para Rust

- tabela das 88 cordas idêntica à da versão C;
- WAV de `--render` com correlação 1,0 com o da versão C (mesmo pico,
  mesmo RMS, afinação igual nas 88 teclas);
- `--bench`: Rust 5,4% do prazo em média com tampo, C 6,2%.

## Roteiro

1. ~~Portar para Rust~~
2. ~~Interface de eventos completa e pedais no instrumento~~
3. Guia de onda digital (corda simulada como ondas viajantes), em andamento
4. Partitura + MusicXML 4.0 simplificado
5. Player: duas mãos, cinco dedos (dedilhado, tempo, dinâmica)
6. GUI em Slint
7. Pé nos pedais, interpretação mais humana

## Licença

Copyright 2026 José Damico <jose.damico@scicrop.com>. Distribuído sob a
[Apache License 2.0](LICENSE).
