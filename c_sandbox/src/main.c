/*
 * Copyright 2026 José Damico <jose.damico@scicrop.com>
 * SPDX-License-Identifier: Apache-2.0
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

/*
 * main.c - piano digital: teclado do terminal -> sintetizador -> ALSA.
 *
 * A thread de áudio só sintetiza e escreve no driver; o teclado roda na
 * thread principal e manda eventos por uma fila sem lock (um produtor,
 * um consumidor), para nunca travar o áudio.
 */
#include "audio.h"
#include "corda.h"
#include "synth.h"
#include "tampo.h"

#include <math.h>
#include <poll.h>
#include <pthread.h>
#include <sched.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

#define TAXA      48000
#define PERIODO   TAMPO_BLOCO   /* 128 quadros por chamada: 2,7 ms a 48 kHz */
#define PRAZO_US  (1e6 * PERIODO / TAXA)

/* ---------- fila de eventos (SPSC) ---------- */

enum { EV_NOTA_ON, EV_NOTA_OFF, EV_ABAFAR };

typedef struct { uint8_t tipo, tecla; float vel; } Evento;

#define FILA_TAM 256
static Evento fila[FILA_TAM];
static atomic_uint fila_ini, fila_fim;

static void enviar(int tipo, int tecla, float vel)
{
    unsigned fim = atomic_load_explicit(&fila_fim, memory_order_relaxed);
    unsigned ini = atomic_load_explicit(&fila_ini, memory_order_acquire);
    if (fim - ini >= FILA_TAM) return;  /* fila cheia: descarta */
    fila[fim % FILA_TAM] = (Evento){ (uint8_t)tipo, (uint8_t)tecla, vel };
    atomic_store_explicit(&fila_fim, fim + 1, memory_order_release);
}

static void consumir(Synth *s)
{
    unsigned ini = atomic_load_explicit(&fila_ini, memory_order_relaxed);
    unsigned fim = atomic_load_explicit(&fila_fim, memory_order_acquire);
    for (; ini != fim; ini++) {
        Evento e = fila[ini % FILA_TAM];
        switch (e.tipo) {
        case EV_NOTA_ON:  synth_nota_on(s, e.tecla, e.vel); break;
        case EV_NOTA_OFF: synth_nota_off(s, e.tecla); break;
        case EV_ABAFAR:   synth_abafar_tudo(s); break;
        }
    }
    atomic_store_explicit(&fila_ini, ini, memory_order_release);
}

/* ---------- medição do tempo de cada bloco ---------- */

static double agora_us(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1e6 + ts.tv_nsec / 1e3;
}

typedef struct {
    double soma, pico;
    long   n;
    long   acima_metade;   /* blocos que usaram mais de 50% do prazo */
} Medidor;

static void medir(Medidor *m, double us)
{
    m->soma += us;
    m->n++;
    if (us > m->pico) m->pico = us;
    if (us > 0.5 * PRAZO_US) m->acima_metade++;
}

/* ---------- vigia: detecta servidor de som travado ---------- */

/* Se o driver/servidor de som parar de consumir amostras, snd_pcm_writei
 * bloqueia para sempre. O vigia observa um contador que avança a cada
 * escrita; parado por 3 s com áudio em uso, o programa sai explicando. */
static atomic_long pulso;
static atomic_int vigiar;
static atomic_int terminal_cru;
static struct termios term_original;

static void restaurar_terminal(void)
{
    if (atomic_exchange(&terminal_cru, 0)) {
        tcsetattr(STDIN_FILENO, TCSANOW, &term_original);
        printf("\033[?25h\n");
        fflush(stdout);
    }
}

static void *thread_vigia(void *arg)
{
    (void)arg;
    long ultimo = -1;
    double desde = 0.0;
    for (;;) {
        usleep(200000);
        if (!atomic_load(&vigiar)) { ultimo = -1; continue; }
        long p = atomic_load(&pulso);
        if (p != ultimo) { ultimo = p; desde = agora_us(); continue; }
        if (agora_us() - desde > 3e6) {
            restaurar_terminal();
            fprintf(stderr,
                "\n\nO áudio parou de responder há 3 s: o servidor de som não está\n"
                "consumindo as amostras (o piano não travou, o dispositivo sim).\n"
                "Teste outro programa:  pw-play /usr/share/sounds/alsa/Front_Center.wav\n"
                "Reiniciar o áudio:     systemctl --user restart pipewire pipewire-pulse wireplumber\n");
            _exit(2);
        }
    }
    return NULL;
}

static void iniciar_vigia(void)
{
    pthread_t th;
    pthread_create(&th, NULL, thread_vigia, NULL);
    pthread_detach(th);
}

/* ---------- configuração e sintetizador global ---------- */

static Synth synth;
static Tampo *tampo;           /* compartilhado; NULL = sem tampo */
static double fator_tensao = 1.0;

static void preparar_synth(void)
{
    synth_init(&synth, TAXA, fator_tensao);
    synth.tampo = tampo;
}

/* ---------- thread de áudio ---------- */

static Audio *audio;
static atomic_int rodando = 1;
static atomic_int vozes_ativas;
static atomic_int rt_ok;
/* estatística da última janela de ~0,5 s, em centésimos de % do prazo */
static atomic_int carga_media, carga_pico;

static void *thread_audio(void *arg)
{
    (void)arg;
    /* prioridade de tempo real: o escalonador não interrompe o áudio para
     * rodar outros programas (precisa de permissão; se falhar, segue normal) */
    struct sched_param sp = { .sched_priority = 70 };
    atomic_store(&rt_ok, pthread_setschedparam(pthread_self(), SCHED_FIFO, &sp) == 0);

    float buf[PERIODO * 2];
    Medidor m = { 0 };
    while (atomic_load(&rodando)) {
        double t0 = agora_us();
        consumir(&synth);
        synth_render(&synth, buf, PERIODO);
        medir(&m, agora_us() - t0);

        if (m.n * PERIODO >= TAXA / 2) {
            atomic_store(&carga_media, (int)(1e4 * m.soma / m.n / PRAZO_US));
            atomic_store(&carga_pico, (int)(1e4 * m.pico / PRAZO_US));
            m = (Medidor){ 0 };
        }
        atomic_store(&vozes_ativas, synth_vozes_ativas(&synth));
        if (audio_escrever(audio, buf, PERIODO) < 0) {
            fprintf(stderr, "erro fatal de áudio\n");
            atomic_store(&rodando, 0);
        }
        atomic_fetch_add(&pulso, 1);
    }
    return NULL;
}

/* ---------- WAV ---------- */

static void escrever_u32(FILE *f, uint32_t v) { fwrite(&v, 4, 1, f); }
static void escrever_u16(FILE *f, uint16_t v) { fwrite(&v, 2, 1, f); }

static int render_wav(const char *caminho)
{
    FILE *f = fopen(caminho, "wb");
    if (!f) { perror(caminho); return 1; }

    preparar_synth();

    /* 88 teclas em sequência, depois um acorde de Dó maior abafado */
    const int passo = TAXA * 35 / 100;
    int total = passo * NUM_TECLAS + TAXA * 6;
    total -= total % PERIODO;
    int acorde[] = { tecla_de_nome("C2"), tecla_de_nome("C3"), tecla_de_nome("E4"),
                     tecla_de_nome("G4"), tecla_de_nome("C5") };
    int t_acorde = passo * NUM_TECLAS + TAXA;

    fwrite("RIFF", 1, 4, f); escrever_u32(f, 36 + total * 4);
    fwrite("WAVEfmt ", 1, 8, f); escrever_u32(f, 16);
    escrever_u16(f, 1); escrever_u16(f, 2);
    escrever_u32(f, TAXA); escrever_u32(f, TAXA * 4);
    escrever_u16(f, 4); escrever_u16(f, 16);
    fwrite("data", 1, 4, f); escrever_u32(f, total * 4);

    float buf[PERIODO * 2];
    int16_t pcm[PERIODO * 2];
    Medidor m = { 0 };
    for (int t = 0; t < total; t += PERIODO) {
        for (int k = 0; k < NUM_TECLAS; k++) {
            int tk = k * passo;
            if (tk >= t && tk < t + PERIODO) synth_nota_on(&synth, k, 0.7f);
            if (tk + passo * 3 >= t && tk + passo * 3 < t + PERIODO) synth_nota_off(&synth, k);
        }
        if (t_acorde >= t && t_acorde < t + PERIODO)
            for (size_t i = 0; i < sizeof acorde / sizeof *acorde; i++)
                synth_nota_on(&synth, acorde[i], 0.8f);
        if (t_acorde + TAXA * 3 >= t && t_acorde + TAXA * 3 < t + PERIODO)
            synth_abafar_tudo(&synth);

        double t0 = agora_us();
        synth_render(&synth, buf, PERIODO);
        medir(&m, agora_us() - t0);
        for (int i = 0; i < 2 * PERIODO; i++)
            pcm[i] = (int16_t)(buf[i] * 32767.0f);
        fwrite(pcm, sizeof(int16_t), 2 * PERIODO, f);
    }
    fclose(f);
    printf("gravado %s (%.1f s, tampo: %s)\n", caminho, (double)total / TAXA,
           tampo ? "sim" : "não");
    printf("tempo por bloco: médio %.1f us, pico %.1f us (prazo %.0f us)\n",
           m.soma / m.n, m.pico, PRAZO_US);
    return 0;
}

/* ---------- benchmark: pior caso de polifonia ---------- */

static void bench_um(const char *rotulo, int com_tampo)
{
    synth_init(&synth, TAXA, fator_tensao);
    synth.tampo = com_tampo ? tampo : NULL;

    /* 32 notas na região mais cara (3 cordas, muitos parciais): G2..D5 */
    for (int i = 0; i < MAX_VOZES; i++)
        synth_nota_on(&synth, tecla_de_nome("G2") + i, 1.0f);

    float buf[PERIODO * 2];
    Medidor m = { 0 };
    int blocos = TAXA * 2 / PERIODO;  /* 2 s de áudio */
    for (int b = 0; b < blocos; b++) {
        double t0 = agora_us();
        synth_render(&synth, buf, PERIODO);
        medir(&m, agora_us() - t0);
    }
    printf("  %-22s médio %7.1f us (%5.1f%%)   pico %7.1f us (%5.1f%%)   "
           "blocos >50%% do prazo: %ld/%ld\n",
           rotulo, m.soma / m.n, 100 * m.soma / m.n / PRAZO_US,
           m.pico, 100 * m.pico / PRAZO_US, m.acima_metade, m.n);
}

static int benchmark(void)
{
    printf("prazo por bloco: %d quadros a %d Hz = %.0f us\n", PERIODO, TAXA, PRAZO_US);
    printf("32 vozes simultâneas, força máxima, 2 s:\n");
    bench_um("só cordas", 0);
    if (tampo) bench_um("cordas + tampo", 1);
    synth_init(&synth, TAXA, fator_tensao);
    synth.tampo = tampo;
    if (tampo) {
        float buf[PERIODO * 2];
        Medidor m = { 0 };
        for (int b = 0; b < TAXA / PERIODO; b++) {
            double t0 = agora_us();
            synth_render(&synth, buf, PERIODO);
            medir(&m, agora_us() - t0);
        }
        printf("  %-22s médio %7.1f us (%5.1f%%)\n", "só tampo (silêncio)",
               m.soma / m.n, 100 * m.soma / m.n / PRAZO_US);
    }
    printf("regra: até ~50%% do prazo no pico, uma thread de áudio basta.\n");
    return 0;
}

/* ---------- tabela das cordas ---------- */

static void imprimir_tabela(void)
{
    Corda c[NUM_TECLAS];
    cordas_init(c, fator_tensao);
    printf("tecla nota   f_alvo(Hz)   L(m)   mu(g/m)    T(N)  f=1/2L*sqrt(T/mu)  "
           "desvio(cents)        B   f1 real   cordas\n");
    for (int k = 0; k < NUM_TECLAS; k++) {
        double f1 = corda_parcial(&c[k], 1);
        printf("%4d  %-4s %10.3f  %6.3f  %8.3f  %7.1f  %14.3f  %+12.2f  %9.6f  %8.3f   %d%s\n",
               k + 1, c[k].nome, c[k].f_alvo, c[k].L, c[k].mu * 1000.0, c[k].T, c[k].f,
               1200.0 * log2(c[k].f / c[k].f_alvo), c[k].B, f1,
               c[k].n_cordas, c[k].enrolada ? " enrolada" : "");
    }
}

/* ---------- teclado do terminal ---------- */

/* duas fileiras como num tracker: z s x d c v g b h n j m , l . ; /
 * (oitava base) e q 2 w 3 e r 5 t 6 y 7 u i 9 o 0 p (oitava acima) */
static int semitom_da_tecla(int ch, int *oitava_extra)
{
    static const char baixo[] = "zsxdcvgbhnjm,l.;/";
    static const char cima[]  = "q2w3er5t6y7ui9o0p";
    const char *p;
    if (ch && (p = strchr(baixo, ch))) { *oitava_extra = 0; return (int)(p - baixo); }
    if (ch && (p = strchr(cima, ch)))  { *oitava_extra = 1; return (int)(p - cima); }
    return -1;
}

/* 1º Ctrl-C: sai normalmente; 2º: sai na hora */
static void ao_sinal(int sig)
{
    (void)sig;
    if (!atomic_exchange(&rodando, 0)) {
        restaurar_terminal();
        _exit(130);
    }
}

static int abrir_audio(const char *disp, unsigned latencia_us)
{
    iniciar_vigia();
    atomic_store(&vigiar, 1);  /* abrir também pode travar */
    audio = audio_abrir(disp, TAXA, latencia_us);
    atomic_fetch_add(&pulso, 1);
    if (!audio) atomic_store(&vigiar, 0);
    return audio ? 0 : -1;
}

static void fechar_audio(void)
{
    audio_fechar(audio);
    atomic_store(&vigiar, 0);
}

static int modo_interativo(const char *disp, unsigned latencia_us)
{
    preparar_synth();
    if (abrir_audio(disp, latencia_us) < 0) return 1;

    tcgetattr(STDIN_FILENO, &term_original);
    struct termios raw = term_original;
    raw.c_lflag &= ~(ICANON | ECHO);
    raw.c_cc[VMIN] = 0;
    raw.c_cc[VTIME] = 0;
    tcsetattr(STDIN_FILENO, TCSANOW, &raw);
    atomic_store(&terminal_cru, 1);
    atexit(restaurar_terminal);
    signal(SIGINT, ao_sinal);

    pthread_t th;
    pthread_create(&th, NULL, thread_audio, NULL);

    printf("\033[?25l"
           "Piano digital - 88 teclas (A0..C8)\n"
           "  z s x d c v g b h n j m , l . ; /   -> oitava base\n"
           "  q 2 w 3 e r 5 t 6 y 7 u i 9 o 0 p   -> oitava de cima\n"
           "  - / =  oitava   [ / ]  força   espaço  abafar   Tab  tampo liga/desliga   Esc  sair\n"
           "  CPU = tempo de síntese / prazo de %.0f us por bloco (média | pico em 0,5 s)\n\n",
           PRAZO_US);

    int oitava = 3;
    float vel = 0.7f;
    char ultima[160] = "";

    while (atomic_load(&rodando)) {
        struct pollfd pfd = { STDIN_FILENO, POLLIN, 0 };
        if (poll(&pfd, 1, 50) > 0) {
            unsigned char ch;
            while (read(STDIN_FILENO, &ch, 1) == 1) {
                int extra;
                int st = semitom_da_tecla(ch, &extra);
                if (st >= 0) {
                    int midi = (oitava + extra + 1) * 12 + st;
                    int k = midi - 21;
                    if (k >= 0 && k < NUM_TECLAS) {
                        enviar(EV_NOTA_ON, k, vel);
                        const Corda *c = &synth.cordas[k];
                        snprintf(ultima, sizeof ultima,
                                 "%-4s f=%.2f Hz  L=%.3f m  T=%.0f N  mu=%.2f g/m  B=%.5f",
                                 c->nome, c->f, c->L, c->T, c->mu * 1000.0, c->B);
                    }
                } else if (ch == '-' && oitava > 0) oitava--;
                else if (ch == '=' && oitava < 7) oitava++;
                else if (ch == '[') vel = fmaxf(0.1f, vel - 0.1f);
                else if (ch == ']') vel = fminf(1.0f, vel + 0.1f);
                else if (ch == ' ') enviar(EV_ABAFAR, 0, 0);
                /* troca de ponteiro é atômica na prática em x86/ARM; a
                 * thread de áudio vê o novo valor no próximo bloco */
                else if (ch == '\t' && tampo) synth.tampo = synth.tampo ? NULL : tampo;
                else if (ch == 27) atomic_store(&rodando, 0);
            }
        }
        printf("\r\033[KC%d  força %.1f  vozes %2d/%d  tampo %s  CPU %4.1f%% | %4.1f%%%s  | %s",
               oitava, vel, atomic_load(&vozes_ativas), MAX_VOZES,
               synth.tampo ? "sim" : "não",
               atomic_load(&carga_media) / 100.0, atomic_load(&carga_pico) / 100.0,
               atomic_load(&rt_ok) ? "  RT" : "", ultima);
        fflush(stdout);
    }

    pthread_join(th, NULL);
    fechar_audio();
    return 0;
}

/* toca uma lista de teclas no dispositivo e sai */
static int tocar(const char *disp, unsigned latencia_us,
                 const int *teclas, int n, float vel, double intervalo_s, double fim_s)
{
    preparar_synth();
    if (abrir_audio(disp, latencia_us) < 0) return 1;

    float buf[PERIODO * 2];
    int passo = (int)(intervalo_s * TAXA);
    int total = passo * n + (int)(fim_s * TAXA);
    for (int t = 0, i = 0; t < total; t += PERIODO) {
        while (i < n && i * passo < t + PERIODO) {
            synth_nota_on(&synth, teclas[i], vel);
            printf("\r\033[K%s  %.2f Hz", synth.cordas[teclas[i]].nome, synth.cordas[teclas[i]].f);
            fflush(stdout);
            i++;
        }
        synth_render(&synth, buf, PERIODO);
        if (audio_escrever(audio, buf, PERIODO) < 0) break;
        atomic_fetch_add(&pulso, 1);
    }
    printf("\n");
    fechar_audio();
    return 0;
}

static void uso(const char *prog)
{
    printf("uso: %s [opções]\n"
           "  (sem opções)        modo interativo pelo teclado\n"
           "  --tabela            mostra L, T, mu e f das 88 cordas\n"
           "  --nota A4 [C#5 ...] toca notas no dispositivo\n"
           "  --demo              toca as 88 teclas em sequência\n"
           "  --render saida.wav  grava as 88 teclas num WAV (sem placa de som)\n"
           "  --bench             mede o tempo de síntese no pior caso (32 vozes)\n"
           "  --sem-tampo         só o som das cordas (como a versão 1)\n"
           "  --tampo ir.wav      usa uma resposta ao impulso gravada\n"
           "  --salvar-tampo x.wav  grava a resposta ao impulso usada\n"
           "  --vel 0.7           força das notas (0..1)\n"
           "  --tensao 1.0        multiplica a tensão de todas as cordas\n"
           "  --dispositivo default   dispositivo ALSA\n"
           "  --latencia 30000    buffer do ALSA em microssegundos\n", prog);
}

int main(int argc, char **argv)
{
    const char *disp = "default";
    unsigned latencia = 30000;  /* 10 ms trava o driver SOF deste notebook (ciclo de 64 amostras) */
    float vel = 0.7f;
    enum { INTERATIVO, TABELA, NOTA, DEMO, RENDER, BENCH } modo = INTERATIVO;
    const char *wav = NULL, *ir_entrada = NULL, *ir_saida = NULL;
    int sem_tampo = 0;
    int notas[NUM_TECLAS], n_notas = 0;

    for (int i = 1; i < argc; i++) {
        const char *a = argv[i];
        int tem_valor = i + 1 < argc;
        if (!strcmp(a, "--tabela")) modo = TABELA;
        else if (!strcmp(a, "--demo")) modo = DEMO;
        else if (!strcmp(a, "--bench")) modo = BENCH;
        else if (!strcmp(a, "--render") && tem_valor) { modo = RENDER; wav = argv[++i]; }
        else if (!strcmp(a, "--nota")) {
            modo = NOTA;
            while (i + 1 < argc && argv[i + 1][0] != '-' && n_notas < NUM_TECLAS) {
                int k = tecla_de_nome(argv[++i]);
                if (k < 0) { fprintf(stderr, "nota inválida: %s\n", argv[i]); return 1; }
                notas[n_notas++] = k;
            }
        }
        else if (!strcmp(a, "--sem-tampo")) sem_tampo = 1;
        else if (!strcmp(a, "--tampo") && tem_valor) ir_entrada = argv[++i];
        else if (!strcmp(a, "--salvar-tampo") && tem_valor) ir_saida = argv[++i];
        else if (!strcmp(a, "--vel") && tem_valor) vel = (float)atof(argv[++i]);
        else if (!strcmp(a, "--tensao") && tem_valor) fator_tensao = atof(argv[++i]);
        else if (!strcmp(a, "--dispositivo") && tem_valor) disp = argv[++i];
        else if (!strcmp(a, "--latencia") && tem_valor) latencia = (unsigned)atoi(argv[++i]);
        else { uso(argv[0]); return !strcmp(a, "--ajuda") || !strcmp(a, "-h") ? 0 : 1; }
    }

    if (modo == TABELA) { imprimir_tabela(); return 0; }

    if (!sem_tampo || modo == BENCH) {
        tampo = ir_entrada ? tampo_de_wav(ir_entrada, TAXA, 3.0f)
                           : tampo_sintetico(TAXA, 1.0f, 12345u);
        if (!tampo) { fprintf(stderr, "não foi possível montar o tampo\n"); return 1; }
        if (ir_saida && tampo_salvar_ir(tampo, ir_saida, TAXA) == 0)
            printf("resposta ao impulso gravada em %s\n", ir_saida);
    }

    int r;
    switch (modo) {
    case RENDER: r = render_wav(wav); break;
    case BENCH:  r = benchmark(); break;
    case NOTA:
        if (!n_notas) { fprintf(stderr, "--nota precisa de pelo menos uma nota\n"); return 1; }
        r = tocar(disp, latencia, notas, n_notas, vel, 0.6, 3.0);
        break;
    case DEMO:
        for (int k = 0; k < NUM_TECLAS; k++) notas[k] = k;
        r = tocar(disp, latencia, notas, NUM_TECLAS, vel, 0.18, 3.0);
        break;
    default:
        r = modo_interativo(disp, latencia);
        break;
    }
    tampo_liberar(tampo);
    return r;
}
