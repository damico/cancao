# Piano digital em C (protótipo)

Sintetiza as 88 teclas de um piano (A0-C8) a partir da física da corda:

    f = (1 / 2L) * √(T / μ)

Cada corda tem comprimento `L`, massa linear `μ` e diâmetro plausíveis para
um piano de cauda. A tensão `T` sai da afinação, como faz um afinador, e a
frequência que soa é calculada pela fórmula. A rigidez do aço adiciona
inarmonicidade aos parciais: `f_n = n*f*√(1 + B*n²)`, com
`B = π³*E*d⁴ / (64*T*L²)`.

## Compilar

    make

Só precisa de `gcc` e `libasound.so.2`, que já vem em qualquer Linux com
som. Os headers do ALSA (`libasound2-dev`) não são necessários.

## Usar

    ./piano                 # modo interativo
    ./piano --tabela        # L, μ, T, f e B das 88 cordas
    ./piano --nota A4 C#5   # toca notas e sai
    ./piano --demo          # as 88 teclas em sequência
    ./piano --render x.wav  # grava as 88 teclas num WAV (sem placa de som)
    ./piano --tensao 1.02   # todas as cordas 2% mais tensas (aproximadamente +17 cents)
    ./piano --bench         # tempo de síntese no pior caso (32 vozes)
    ./piano --sem-tampo     # só as cordas, sem tampo/sala (como a v1)
    ./piano --tampo ir.wav  # usa uma resposta ao impulso gravada
    ./piano --salvar-tampo ir.wav   # grava a resposta ao impulso usada

Teclado no modo interativo:

    z s x d c v g b h n j m , l . ; /    oitava base
    q 2 w 3 e r 5 t 6 y 7 u i 9 o 0 p    oitava de cima
    - / =  muda a oitava     [ / ]  força     espaço  abafa
    Tab    liga/desliga o tampo (comparação A/B)        Esc  sai

A linha de status mostra `CPU média | pico`: quanto do prazo de cada bloco
de áudio (128 quadros = 2,67 ms) a síntese consumiu no último meio
segundo. `RT` aparece se a thread de áudio conseguiu prioridade de tempo
real (`SCHED_FIFO`). Até ~50% no pico, uma thread de áudio basta; acima
disso vale dividir as vozes entre núcleos.

Se der estalos ou travar, aumente o buffer: `--latencia 50000`. Para listar
dispositivos: `aplay -L`. Detalhes em [Áudio: latência, driver e
hardware](#áudio-latência-driver-e-hardware).

## Áudio: latência, driver e hardware

### Situação atual (out/2026, Dell XPS 9315)

O padrão é `--latencia 30000` (30 ms) por limitação do caminho de áudio do
notebook, não do programa: a síntese usa ~15% do prazo de cada bloco
(`./piano --bench`).

Caminho do som:

    piano -> ALSA -> PipeWire -> driver SOF (kernel) -> DSP Intel (firmware SOF)
          -> barramento SoundWire -> codec/amplificador (DAC) -> alto-falante

Medições (`pw-top` mostra o ciclo com que o PipeWire aciona o hardware):

| `--latencia` | ciclo do PipeWire no alto-falante | resultado          |
|--------------|-----------------------------------|--------------------|
| 10000        | 64 amostras (1,3 ms)              | travou na 1ª vez   |
| 30000        | 256 amostras (5,3 ms)             | 8/8 sem erro       |
| 100000       | -                                 | 6/6 sem erro       |

Com ciclos de 1,3 ms o driver SOF perde um prazo (*underrun*) e **não
consegue se recuperar**: o PipeWire repete no log
`spa.alsa: hw:sofsoundwire,2p: snd_pcm_avail after recover: Broken pipe` e
o alto-falante para de tocar para **todos** os programas (`pw-play`,
`aplay`, `paplay` também travam) até reiniciar o servidor de som:

    systemctl --user restart pipewire pipewire-pulse wireplumber

Não é o DAC nem defeito físico: o problema está no firmware/driver do DSP
Intel SOF (funciona com buffer maior e volta ao reiniciar o PipeWire).
Atualizações de kernel e de `linux-firmware`/`sof-firmware` podem corrigir.

Proteções no programa:
- um vigia encerra o piano com uma explicação se o áudio ficar 3 s parado
  (em vez de congelar);
- o fechamento usa `snd_pcm_drop` (não `drain`, que trava com o servidor parado);
- 2º Ctrl-C força a saída; o terminal é sempre restaurado.

Diagnóstico rápido:

    journalctl --user -u pipewire --since "-10min" | grep "Broken pipe"
    pw-top                                   # ciclo (QUANT) e erros (ERR) por nó
    pw-play /usr/share/sounds/alsa/Front_Center.wav   # o sistema toca?

### Meta para um piano digital

Latência total abaixo de ~10 ms (instrumentos comerciais ficam em 3-7 ms).
A 30 ms a resposta da tecla já parece "mole". Componentes da latência:
leitura da tecla + bloco de síntese (2,7 ms) + buffer do driver + conversor.

### Quando houver uma interface de áudio profissional

1. **Hardware:** interface USB *class compliant* (USB Audio Class 2.0), que
   funciona no Linux sem driver próprio e não passa pelo DSP Intel.
2. **Prioridade de tempo real:** o sistema já libera `rtprio 95` para o
   grupo `pipewire` (`/etc/security/limits.d/25-pw-rlimits.conf`):

       sudo usermod -aG pipewire $USER   # depois, sair e entrar na sessão

   O `RT` deve aparecer na linha de status do modo interativo.
3. **Medir antes de baixar o buffer:** repetir a tabela acima com a
   interface (`--latencia 10000`, `5000`...), observando `pw-top` e o log.
   Pelo PipeWire, a interface aparece em `aplay -L`; use `--dispositivo`.
4. **Acesso direto ao hardware (opcional):** `--dispositivo hw:X,Y` ou
   `plughw:X,Y` evita o PipeWire e dá a menor latência, mas o piano passa a
   ocupar o dispositivo sozinho (outros programas ficam sem som nele).
   Alternativa: fixar o ciclo do PipeWire, ex.
   `pw-metadata -n settings 0 clock.force-quantum 128`.
5. **Próximo passo de código:** entrada por teclado MIDI (ALSA sequencer),
   que dá nota solta e velocidade real, e reduz a latência da leitura da
   tecla em relação ao terminal.

## Arquitetura

| arquivo            | função                                                        |
|--------------------|---------------------------------------------------------------|
| `src/corda.c`      | física: L, μ, T, f, B de cada tecla                           |
| `src/synth.c`      | síntese modal polifônica (32 vozes, até 48 parciais × 3 cordas) |
| `src/tampo.c`      | tampo + sala: convolução particionada com FFT própria         |
| `src/audio_alsa.c` | saída ALSA, buffer de ~30 ms                                  |
| `src/main.c`       | thread de áudio, fila de eventos sem lock, teclado, WAV       |

Detalhes do som:
- O martelo bate a 1/8 da corda, por isso parciais múltiplos de 8 quase somem.
- O feltro do martelo funciona como um passa-baixa: quanto mais forte a nota, mais brilho.
- O decaimento tem dois estágios. Os graves soam por mais tempo e os parciais agudos morrem antes.
- Cada nota tem 1 a 3 cordas levemente desafinadas entre si, o que produz o batimento do uníssono.
- Do F#6 para cima não há abafadores, como num piano real.

## Limitações e próximos passos

- O terminal não informa quando a tecla é solta, então as notas decaem
  sozinhas e o espaço abafa tudo. Um teclado MIDI (ALSA rawmidi/seq)
  resolveria isso e daria a velocidade real de cada nota.
- O tampo usa uma resposta ao impulso sintética (300 modos de placa +
  cauda de sala). Uma resposta gravada de um piano real (`--tampo`) soa melhor.
- Faltam pedal de sustain e ressonância simpática entre cordas.
- Para timbre mais realista dá para trocar a síntese modal por guias de onda
  digitais (Karplus-Strong estendido com filtros de dispersão).
