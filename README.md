# Canção: Um piano digital por modelagem física

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

    sudo apt install libasound2-dev libfontconfig1-dev pkg-config   # só no Linux, uma vez
    cargo build --release
    cargo test --release

Sem a interface gráfica (dispensa o `libfontconfig1-dev`):

    cargo build --release --no-default-features --features audio

Sem os headers do ALSA, dá para compilar só a parte offline
(`--tabela`, `--render`, `--bench`):

    cargo build --release --no-default-features

## Tocar um MusicXML (início rápido)

    cargo build --release
    ./target/release/piano --musicxml exemplos/ode.musicxml

O player toca o tema da *Ode à Alegria* (Beethoven, domínio público) com as
duas mãos e mostra, para cada momento, a mão (`MD`/`ME`) e o dedo de cada
nota (1 = polegar ... 5 = mínimo):

    Ode à Alegria (tema)  (humanização 0.5)
        0.0 s  ME C3(5) E3(3) G3(1)
        0.0 s  MD E4(2)
        0.6 s  MD E4(2)
        1.3 s  MD F4(3)
        ...

Variações úteis:

    ./target/release/piano --musicxml exemplos/ode.musicxml --modelo guia      # corda em guia de onda
    ./target/release/piano --musicxml exemplos/ode.musicxml --humanizar 0      # tempos exatos
    ./target/release/piano --musicxml exemplos/ode.musicxml --render ode.wav   # grava WAV, sem placa de som

Qualquer arquivo `.musicxml` não comprimido funciona, dentro do recorte
descrito em [MusicXML](#musicxml).

## Interface: piano roll

![Piano roll com a partitura compacta: notas que soam em dourado nas duas vistas, dedo de cada nota, inspetor, velocity e sustain](docs/piano-roll.png)

    ./target/release/piano --gui                               # abre exemplos/ode.musicxml
    ./target/release/piano --gui --musicxml arq.musicxml       # qualquer MusicXML
    ./target/release/piano --gui --partitura acordes           # exemplos embutidos

Visualizador temporal da partitura tocada pelo player, inspirado no piano
roll de DAWs, em [Slint](https://slint.dev):

- barra superior: abrir, tocar, pausar, parar, andamento, posição (compasso e
  tempo), fórmula de compasso, zoom, grade, legenda de cores, cores por voz
  ou por mão, modelo de corda;
- régua de compassos (clique para ir para aquele ponto);
- teclado vertical das 88 teclas (as que soam ficam douradas);
- roll: cada nota com a cor da voz (ou da mão) e o dedo escolhido pelo
  player; as já tocadas ficam apagadas, as que soam brilham; playhead
  dourado;
- inspetor: nota, frequência, compasso, duração, voz, mão, dedo, velocity;
- áudio: escolha da saída (placa de som) e volume de 0 a 150%;
- faixas de velocity (força tocada pelo player) e de sustain.

| gesto                 | efeito                                 |
|-----------------------|----------------------------------------|
| roda                  | rolar as teclas                        |
| Shift + roda          | andar no tempo                         |
| Ctrl + roda           | zoom no tempo, no ponto do cursor      |
| botões - e +          | zoom no tempo, 25% por clique          |
| Ctrl + Shift + roda   | altura das linhas das teclas           |
| clique na régua       | ir para o ponto (tocando ou não)       |
| clique numa nota      | detalhes no inspetor                   |
| botão Abrir           | escolher um `.musicxml`/`.xml` e tocar |
| Áudio, Saída          | trocar a placa de som (pausa; Tocar continua dali) |
| Áudio, Volume         | volume geral, na hora                  |

O botão **Abrir** usa o diálogo de arquivos do sistema (no Linux, o do
`xdg-desktop-portal`, que o Ubuntu já traz; no macOS, o nativo). A peça
aberta passa pelo player e começa a tocar; se o arquivo não puder ser lido,
a peça atual continua e o erro aparece no painel inferior direito.

O relógio do áudio manda: a cada quadro (16 ms) a interface lê a posição do
motor e desenha o playhead ali. Só as notas dentro da área visível são
entregues ao Slint (viewport culling), e os eventos vão para o motor só 2 s
à frente, então obras longas não enchem a fila. Sem dispositivo de áudio a
interface funciona igual, sem som.

### Partitura tradicional (Live Score Panel)

![Partitura expandida sobre o piano roll, sincronizadas](docs/partitura-expandida.png)

![Partitura em tela cheia](docs/partitura-tela-cheia.png)

Acima do piano roll fica a partitura tradicional (claves, compassos,
ligaduras, pausas, dinâmicas, articulações), gravada pelo
[Verovio](https://www.verovio.org) a partir do mesmo MusicXML e sincronizada
com o mesmo relógio do motor de áudio:

- **Ocultar / Compacto / Expandir / Tela cheia**: o compacto mostra o
  sistema atual; o expandido, vários (arraste a divisória para ajustar); a
  tela cheia esconde o piano roll. Trocar de modo nunca interrompe o som.
- **Seguir execução**: a partitura acompanha o playhead sistema a sistema,
  mostrando o próximo antes de a linha acabar; também segue os saltos.
- **- / +**: tamanho da notação (a partitura é diagramada de novo).
- As notas que soam ficam em dourado; a selecionada, em azul.
- **Navegação nos dois sentidos**: clicar numa nota da partitura leva o
  playhead (e o roll) até ela; clicar numa nota do roll a seleciona e a
  mostra na partitura.

Como funciona: ao abrir a peça, o Verovio diagrama a partitura numa thread
à parte (a Sonata de Liszt, 129 sistemas, leva cerca de 1,5 s), um SVG por
sistema; nada é redesenhado a cada nota. Os sistemas são rasterizados com
`resvg` só quando aparecem, e ficam em cache. Cada nota desenhada é ligada à
nota que o player toca por (compasso, posição no compasso, tecla), o que
mantém a ligação mesmo quando o Verovio e o nosso leitor discordam de alguma
duração no arquivo: na Sonata de Liszt, 94,5% das notas desenhadas ficam
ligadas (as demais são ornamentos e notas que o leitor simplificado ignora).

**Licença do Verovio**: o Verovio é LGPL-3.0 e é compilado junto com o
programa (pelo `verovioxide`, Apache-2.0). Quem distribuir binários com a
partitura precisa permitir trocar o Verovio por outra versão (o código do
cancao é público e compila pelo Cargo, o que atende a isso). Para um binário
só com código Apache, compile sem a feature `partitura`:

    cargo build --release --no-default-features --features gui

A primeira compilação com a partitura baixa o código do Verovio do GitHub e
o compila (cerca de 10 minutos); as seguintes usam o cache.

Captura sem janela (renderizador por software do Slint), útil para
documentação e testes (as imagens deste README foram geradas assim, com
`exemplos/ode.musicxml`):

    ./target/release/piano --gui-captura roll.png --gui-posicao 9.5   # playhead na semínima 9,5
    ./target/release/piano --gui-captura roll.png --gui-partitura expandida   # oculta, compacta, expandida, cheia

## Usar

    ./target/release/piano                  # modo interativo
    ./target/release/piano --tabela         # L, μ, T, f e B das 88 cordas
    ./target/release/piano --nota A4 C#5    # toca notas e sai
    ./target/release/piano --demo           # as 88 teclas em sequência
    ./target/release/piano --pedais         # demonstração dos 3 pedais
    ./target/release/piano --render x.wav   # grava as 88 teclas num WAV
    ./target/release/piano --render x.wav --pedais   # grava a demonstração
    ./target/release/piano --musicxml arq.musicxml   # o player toca um MusicXML
    ./target/release/piano --partitura escala        # partituras embutidas: escala, acordes, desafios
    ./target/release/piano --humanizar 0.5           # variações humanas do player (0 = exato)
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
  ponta próxima apaga os harmônicos múltiplos de 8 sozinha. O golpe vem
  de um martelo simulado (`src/martelo.rs`): massa contra a corda, feltro
  como mola não linear `F = K*compressão^p`, corda resistindo com a
  impedância `Z = √(T*μ)`. A rigidez do
  aço entra por uma cascata de passa-tudos (até 32 seções, projetada por
  tecla ao criar o instrumento): os agudos dão a volta mais rápido e os
  parciais caem em `f_n = n*f*√(1 + B*n²)`. As cordas do uníssono passam
  pelos filtros em paralelo (SIMD) e terminam no mesmo cavalete: ele se
  move com a soma das ondas e devolve esse movimento a todas, o que produz
  o decaimento em dois estágios (cordas em fase perdem energia rápido, em
  oposição ficam; Weinreich, 1977).

Etapas do guia de onda:

1. ~~Laço afinado por L, T e μ, perdas, martelo como pulso~~ (afinação
   medida nas 88 teclas: no máximo 1,2 cent)
2. ~~Rigidez: filtros de dispersão (inarmonicidade B)~~ (parciais medidos
   no som gerado: 1,6 a 4,7 cents RMS de `f_n`, contra 32 a 55 cents sem
   rigidez)
3. ~~Martelo não linear (feltro como mola dura)~~ (massa, K e p de
   Chaigne e Askenfelt 1994; o brilho cresce com a força sozinho: no C4,
   de 0,45% da energia acima de 1,5 kHz em pp para 10,9% em ff)
4. ~~Cordas do uníssono acopladas no cavalete~~ (decaimento em dois
   estágios medido: A4 cai 13 dB/s no início e 2,3 dB/s no pós-som, contra
   1,9 dB/s previstos pela perda própria da corda; com una corda, a corda
   não atingida vibra só pelo cavalete)
5. Ressonância simpática entre notas diferentes (cordas livres com o
   sustain, excitadas pelo cavalete)

A calibrar no martelo: nos graves o contato sai com ~1,5 ms em mf, abaixo
dos 2 a 4 ms da literatura.

## MusicXML

Leitor simplificado de MusicXML 4.0 (`src/musicxml.rs`). Entende:

- `score-partwise` não comprimido (`.musicxml` ou `.xml`);
- notas com `pitch` (`step`, `alter`, `octave`) e `duration`, pausas,
  `chord`, `backup`, `forward` e mudanças de `divisions`;
- `staff`: 1 = pauta de cima (mão direita), 2 = pauta de baixo; sem
  `staff`, a 1ª parte vai para cima e as demais para baixo;
- ligaduras de prolongamento (`tie` start/stop): viram uma nota só;
- andamento (`sound tempo`), dinâmicas (`pppp` a `ffff`) e pedal de sustain
  (`pedal` start/stop/change).

Ignora sem erro: `voice`, notas de ornamento (`grace`), articulações,
ornamentos, repetições, `offset` de direções, letras e diagramação. Não lê
`score-timewise` nem o formato comprimido `.mxl`.

O exemplo `exemplos/ode.musicxml` usa todos esses elementos.

## Player

O player (`src/player.rs`) é o pianista: lê uma `Partitura` (de um MusicXML
ou embutida) e decide como um humano a tocaria, com duas mãos de cinco
dedos. O instrumento não sabe de partitura e o player não sabe de áudio:
entre os dois só passam eventos.

    MusicXML -> Partitura -> Player -> Roteiro (eventos com horário) -> instrumento

1. **Mãos:** pauta de cima para a direita, de baixo para a esquerda. No
   máximo 5 notas por mão ao mesmo tempo; o excesso passa para a outra mão
   se ela estiver livre, senão é descartado com aviso.
2. **Dedos** (`src/dedilhado.rs`): modelo ergonômico de Parncutt et al.
   (1997), com aberturas relaxada, confortável e praticável para cada par de
   dedos de uma mão adulta e regras de dificuldade (estiramento, dedo fraco,
   polegar na tecla preta, passagem do polegar, mudança de posição da mão),
   resolvido por programação dinâmica. Para acordes, que o modelo original
   não cobre, o anelar custa um pouco mais (é o dedo menos independente) e o
   salto da mão entre acordes é normal. Resultados conferidos em teste: a
   escala de Dó de duas oitavas na mão direita sai exatamente como se ensina
   (1-2-3-1-2-3-4-1-2-3-1-2-3-4-5 e volta), tríades em 1-3-5 e 1-2-5,
   oitavas no baixo em 5-1.
3. **Corpo:** acorde que não cabe na mão é arpejado (com aviso); um dedo só
   abaixa outra tecla depois de soltar a anterior; uma tecla só é tocada de
   novo depois de solta.
4. **Tempo e força:** semínimas viram segundos pelo andamento; a
   humanização (`--humanizar`, 0 = exato e repetível) varia levemente tempo e
   força e destaca a nota mais aguda da mão direita (a melodia).
5. **Pé (versão 0):** executa as marcações de pedal da partitura, abaixando
   60 ms depois do tempo (pedal sincopado). Decidir sozinho quando pedalar
   fica para uma próxima etapa.

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
| `src/partitura.rs` | o que está escrito: notas, pautas, andamento, pedal; exemplos |
| `src/musicxml.rs` | leitor simplificado de MusicXML 4.0                        |
| `src/player.rs`  | o pianista: mãos, corpo, tempo, força, pé                  |
| `src/dedilhado.rs` | dedilhado pelo modelo ergonômico de Parncutt            |
| `src/gui/linha_do_tempo.rs` | posições, zoom, grade e o que está visível (testável sem janela) |
| `src/gui/transporte.rs` | tocar, pausar, parar e saltar, alimentando o motor aos poucos |
| `src/gui/mod.rs` | liga a interface ao player e ao motor; captura em PNG     |
| `ui/piano_roll.slint` | componentes visuais do piano roll e do painel da partitura |
| `src/notacao.rs` | partitura pelo Verovio: sistemas, posições das notas, ligação com a `Partitura` |
| `src/gui/painel.rs` | painel da partitura: diagramação em segundo plano, cache, rolagem |
| `src/motor.rs`   | agenda eventos por quadro, blocos de 128, medição de tempo |
| `src/synth.rs`   | instrumento: vozes, teclas, abafadores, pedais, cavalete e tampo |
| `src/modal.rs`   | corda por síntese modal (48 parciais × 3 cordas)            |
| `src/guia.rs`    | corda por guia de onda digital                              |
| `src/martelo.rs` | martelo de feltro: massa e mola não linear contra a corda   |
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
4. ~~Partitura + MusicXML 4.0 simplificado~~ (versão inicial)
5. ~~Player: duas mãos, cinco dedos (dedilhado, tempo, dinâmica)~~ (versão inicial)
6. ~~GUI em Slint~~ (piano roll, primeira versão: visualização e transporte)
7. Pé nos pedais, interpretação mais humana

## Pendências

O que ainda precisa ser resolvido, por área. Os números vêm das medições
feitas durante o desenvolvimento.

### Áudio e plataformas

- **Latência alta no notebook de desenvolvimento.** O buffer padrão é de
  30 ms porque com 10 ms o driver Intel SOF/SoundWire trava (ver
  [`c_sandbox/README.md`](c_sandbox/README.md#áudio-latência-driver-e-hardware)).
  Um piano digital pede menos de 10 ms: medir de novo com uma interface de
  áudio USB e só então baixar o padrão.
- **Sem prioridade de tempo real.** O usuário de desenvolvimento não tem
  `rtprio` (`RT` não aparece no modo interativo); entrar no grupo `pipewire`
  resolve no Ubuntu. Sem isso, picos de carga da máquina podem causar
  estalos.
- **macOS não testado.** Falta conferir o buffer do CoreAudio pelo `cpal` e
  a prioridade da thread de áudio (lá `SCHED_FIFO` via `libc` não é o
  caminho usual). Windows também não testado.
- **Vigia do áudio não exercitado na versão Rust.** O código é o mesmo da
  versão C, testada, mas só daria para provar travando o driver de novo.
- **Modo interativo não testado automaticamente**, inclusive a detecção do
  protocolo de teclado do kitty (tecla solta) e os pedais pelo teclado.
- `cpal` fixado na 0.15.3 (a atual é a 0.18); atualizar com calma.
- `--listar` imprime avisos da biblioteca ALSA (`dmix plugin supports only
  playback`) antes da lista. Cosmético.

### Som

- **Ressonância simpática entre notas** (etapa 5 do guia de onda): com o
  sustain, as cordas livres de outras notas deveriam vibrar pelo cavalete.
- **Desafinação do uníssono no guia de onda** é bem menor que no modal (0,2 e
  -0,13 cent contra 0,9 e -0,6): os dois estágios só aparecem com
  desafinação pequena perto do acoplamento. Efeitos: quase não há batimento
  audível e o pós-som dos agudos é fraco (C6 e C8 ficam 9 a 11 dB abaixo do
  modal entre 1 e 1,5 s). Um cavalete com resposta reativa (não só
  resistiva), como o real, misturaria os modos de outra forma; calibrar com
  gravações de piano.
- **Notas de uma corda (A0 a E1)** continuam com decaimento único: o pós-som
  delas vem das duas polarizações da corda (vertical e horizontal), não
  modeladas.
- **Custo:** o acoplamento dobrou o custo do guia de onda (de ~4% para ~8,5%
  do prazo do bloco com 32 vozes); aceito em troca da qualidade.
- **Martelo nos graves:** contato de ~1,5 ms em mf, abaixo dos 2 a 4 ms da
  literatura. Calibrar massa/feltro abaixo do C2 (hoje extrapolados).
- **Ataque dos agudos:** dobrar o golpe numa volta removeu o estalo
  artificial, mas também o "toc" real do martelo e da mecânica nos agudos,
  que hoje é só o ruído genérico de 4 ms.
- **Dispersão:** erro de até ~6 cents RMS nos graves e nos agudos extremos
  (limites da cascata de 1ª ordem e do atraso de uma volta); os graves usam
  até 30 seções, o que pesa no processamento.
- **Tampo sintético.** A resposta ao impulso é gerada (300 modos de placa e
  cauda de sala); uma resposta gravada de um piano real (`--tampo`) deve
  soar melhor.
- **Fenômenos não modelados:** parciais fantasmas dos graves (vibração
  longitudinal), as duas polarizações da corda, abafador que abafa igual em
  todas as frequências.
- **Tudo igual a cada toque:** o desafino do uníssono é o mesmo nas 88
  teclas e a mesma nota repetida gera exatamente o mesmo som. Falta
  variação por tecla e por golpe.
- **Volume ajustado à mão:** a força da tecla entra como `vel^1.6` e há uma
  compensação empírica dos agudos por modelo; não sai da física.
- **Modal:** limitado a 48 parciais, os graves ficam pobres.
- **Acordes fortes chegam perto do limite** (pico de ~30 mil em 32767 no
  acorde final de `--render`); a saturação suave segura, mas distorce.

### Motor e entrada

- **Eventos com resolução de bloco:** são aplicados no início do bloco que
  contém o instante (2,7 ms), não na amostra exata.
- **Taxa fixa em 48 kHz.** A linha de atraso tem no máximo 2048 amostras,
  o que não comporta o A0 a 96 kHz.
- **Sem entrada MIDI real.** A tradução das mensagens existe e é testada
  (`eventos::de_midi`), mas falta ler um teclado MIDI (ALSA sequencer, e o
  equivalente no macOS).
- **Terminais sem aviso de tecla solta** (ex.: GNOME Terminal): a nota é
  solta sozinha após 0,5 s e os pedais viram liga/desliga.

### Player e MusicXML

- **Mão esquerda em escalas longas:** na escala de duas oitavas sai um
  dedilhado tocável mas não o pedagógico (4-3-2-1-3-2-1-3-2-1-4-3-2-1 em vez
  de 5-4-3-2-1-3-2-1-4-3-2-1-3-2-1). O modelo de Parncutt não tem, por
  exemplo, a preferência por começar uma escala longa com o dedo da ponta.
- **Dedilhado escrito ignorado:** o `<fingering>` do MusicXML ainda não é
  lido; o player sempre escolhe sozinho.
- **Mãos só pela pauta:** não há divisão inteligente (ex.: uma voz da pauta
  de cima tocada pela esquerda), só o excesso de 5 notas muda de mão.
- **Corpo simplificado:** o dedilhado considera só o momento anterior para
  saber se um dedo está ocupado; notas longas seguradas enquanto a mesma mão
  toca outras podem ser encurtadas. Não há limite de velocidade da mão em
  saltos grandes.
- **Pé:** só executa as marcações; não decide o pedal sozinho. Sem una corda
  e sostenuto vindos da partitura.
- **MusicXML:** sem `.mxl`, `score-timewise`, repetições, ornamentos,
  articulações (staccato, acento) e vozes; dinâmicas valem para a parte toda
  (as duas pautas) e não há crescendo gradual (`wedge`).

### Interface (piano roll)

- **Primeira versão só visualiza:** editar notas (arrastar altura e posição,
  mudar duração com quantização) e desenhar velocity e pedal ficam para a
  segunda etapa.
- **Saídas de áudio:** a lista é a do `cpal`; no Linux ela inclui
  dispositivos `hw:`/`plughw:` que o PipeWire costuma manter ocupados (dão
  erro ao escolher) e entradas sem uso prático. A escolha e o volume não são
  lembrados entre execuções.
- **Abrir arquivo:** o diálogo bloqueia a interface enquanto está aberto
  (a reprodução é pausada antes); não há arrastar e soltar nem lista de
  arquivos recentes; `.mxl` comprimido não abre.
- **Interação não testada automaticamente:** rolagem, zoom, cliques e a
  sincronia do playhead com o som foram conferidos só pelas capturas e por
  abrir a janela; o sentido da roda pode estar invertido em algum sistema.
- **Tempo em semínimas (f64):** quiálteras e divisões complexas do MusicXML
  ficam com arredondamento; uma representação racional (ticks) seria mais
  exata.
- **Notas escritas, não tocadas:** o roll mostra a posição escrita; a
  humanização do player (alguns ms) não aparece.
- **Obras muito grandes:** os modelos do Slint são refeitos quando algo
  visível muda; com dezenas de milhares de notas na tela pode ser preciso
  rasterizar as camadas fixas em imagem.
- **Camada das cordas vibrando** (o diferencial planejado: mostrar o que
  ainda soa pelo pedal e pelo modelo físico) ainda não existe.

### Partitura tradicional

- **Repetições, casas de 1ª e 2ª vez:** nem o leitor nem o Verovio (como
  configurado) expandem repetições; a partitura e a execução tocam o que está
  escrito, uma vez.
- **Notas não ligadas:** ornamentos e notas que o leitor simplificado ignora
  aparecem na partitura mas não são destacadas nem clicáveis para navegar.
  Nas 235 partituras da ASAP dataset: média de 98,5% das notas desenhadas
  ligadas (mediana 99,8%); as piores são a Mephisto Waltz de Liszt (52,7%) e
  a Sonata Hob. XVI:39/2 de Haydn (77,5%), ainda não investigadas.
- **Diagramação:** o Verovio às vezes sobrepõe textos (número do compasso e
  indicação de andamento no início do sistema); a divisória do modo
  expandido não é lembrada entre execuções.
- **Primeira compilação lenta** (o Verovio em C++, ~10 min) e a licença
  LGPL-3.0 do Verovio para quem distribuir binários (ver acima).

### Próximas etapas do roteiro

Segunda etapa da interface (edição) e a camada das cordas vibrando.

## Licença

Copyright 2026 José Damico <jose.damico@scicrop.com>. Distribuído sob a
[Apache License 2.0](LICENSE).
