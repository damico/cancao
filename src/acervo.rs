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

//! Acervo: uma pasta com muitas partituras (por exemplo o PDMX, com nomes de
//! arquivo que são só códigos) vira um catálogo pesquisável por título,
//! compositor e instrumento.
//!
//! - [`Acervo::indexar`] lê só o cabeçalho de cada MusicXML (título,
//!   compositor, partes e pautas), em paralelo;
//! - o índice fica num arquivo de texto no cache do usuário, para abrir na
//!   hora das próximas vezes;
//! - [`Acervo::buscar`] procura sem diferenciar maiúsculas nem acentos: cada
//!   palavra digitada precisa aparecer, e começo de palavra no título ou no
//!   compositor vale mais;
//! - [`Config`] guarda a pasta escolhida e as partituras abertas por último.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use crate::musicxml;

/// Quanto do MusicXML se lê para achar o cabeçalho.
const CABECALHO: u64 = 64 * 1024;
/// Primeira linha do arquivo de índice (muda se o formato mudar).
const VERSAO: &str = "cancao-acervo 2";
/// Partituras recentes guardadas.
const RECENTES: usize = 12;

/// Uma partitura do acervo.
#[derive(Clone, Debug, PartialEq)]
pub struct Obra {
    /// caminho relativo à raiz do acervo
    pub caminho: PathBuf,
    pub compositor: String,
    pub titulo: String,
    /// nomes das partes, separados por vírgula
    pub instrumentos: String,
    pub partes: u32,
    /// pautas da primeira parte (2 = piano)
    pub pautas: u32,
    /// compositor, título e instrumentos normalizados (para a busca),
    /// separados por '|'
    chave: String,
}

impl Obra {
    fn nova(caminho: PathBuf, compositor: String, titulo: String, instrumentos: String, partes: u32, pautas: u32) -> Obra {
        let chave = format!("{}|{}|{}", normalizar(&compositor), normalizar(&titulo), normalizar(&instrumentos));
        Obra { caminho, compositor, titulo, instrumentos, partes, pautas, chave }
    }

    /// Para piano (ou teclado): todas as partes têm nome de teclado, ou uma
    /// parte só com duas pautas.
    pub fn piano(&self) -> bool {
        const TECLADOS: [&str; 7] = ["piano", "klavier", "clavier", "keyboard", "cembalo", "harpsichord", "teclado"];
        let nomes = self.chave.rsplit('|').next().unwrap_or("");
        let todas = !nomes.is_empty() && nomes.split(',').all(|n| TECLADOS.iter().any(|t| n.contains(t)));
        todas || (self.partes == 1 && self.pautas == 2)
    }
}

/// Andamento de uma indexação (para a interface mostrar).
#[derive(Default)]
pub struct Progresso {
    pub feitos: AtomicUsize,
    pub total: AtomicUsize,
}

#[derive(Clone, Debug, Default)]
pub struct Acervo {
    pub raiz: PathBuf,
    pub obras: Vec<Obra>,
}

impl Acervo {
    /// Lê o cabeçalho de todos os MusicXML da pasta (e subpastas).
    /// Arquivos ilegíveis ficam de fora.
    pub fn indexar(raiz: &Path, progresso: &Progresso) -> Result<Acervo, String> {
        let mut arquivos = Vec::new();
        listar(raiz, &mut arquivos).map_err(|e| format!("{}: {e}", raiz.display()))?;
        arquivos.sort();
        progresso.total.store(arquivos.len(), Relaxed);
        let proximo = AtomicUsize::new(0);
        // metade dos núcleos e prioridade baixa: o resultado é o mesmo, só
        // demora mais, e a síntese (que não pode perder o prazo de cada
        // bloco) continua com processador sobrando
        let fios = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1));
        let mut obras: Vec<(usize, Obra)> = std::thread::scope(|s| {
            let tarefas: Vec<_> = (0..fios)
                .map(|_| {
                    s.spawn(|| {
                        prioridade_baixa();
                        let mut minhas = Vec::new();
                        loop {
                            let i = proximo.fetch_add(1, Relaxed);
                            let Some(arquivo) = arquivos.get(i) else { break };
                            if let Some(o) = ler_obra(raiz, arquivo) {
                                minhas.push((i, o));
                            }
                            progresso.feitos.fetch_add(1, Relaxed);
                        }
                        minhas
                    })
                })
                .collect();
            tarefas.into_iter().flat_map(|t| t.join().unwrap_or_default()).collect()
        });
        obras.sort_by_key(|(i, _)| *i);
        Ok(Acervo { raiz: raiz.to_path_buf(), obras: obras.into_iter().map(|(_, o)| o).collect() })
    }

    /// O índice salvo para esta pasta, ou uma indexação nova (que é salva).
    pub fn abrir(raiz: &Path, progresso: &Progresso, reindexar: bool) -> Result<Acervo, String> {
        let indice = arquivo_de_indice(raiz);
        if !reindexar
            && let Some(a) = std::fs::read_to_string(&indice).ok().and_then(|t| Acervo::de_texto(&t))
            && a.raiz == raiz
        {
            return Ok(a);
        }
        let a = Acervo::indexar(raiz, progresso)?;
        if let Some(pasta) = indice.parent() {
            let _ = std::fs::create_dir_all(pasta);
        }
        let _ = std::fs::write(&indice, a.para_texto());
        Ok(a)
    }

    /// Índice em texto: a versão e a raiz, depois uma obra por linha com os
    /// campos separados por tabulação.
    pub fn para_texto(&self) -> String {
        let mut s = format!("{VERSAO}\t{}\n", self.raiz.display());
        for o in &self.obras {
            s += &format!("{}\t{}\t{}\t{}\t{}\t{}\n", o.caminho.display(), o.compositor, o.titulo, o.instrumentos, o.partes, o.pautas);
        }
        s
    }

    pub fn de_texto(texto: &str) -> Option<Acervo> {
        let mut linhas = texto.lines();
        let raiz = linhas.next()?.strip_prefix(VERSAO)?.strip_prefix('\t')?;
        let obras = linhas
            .filter_map(|l| {
                let c: Vec<&str> = l.split('\t').collect();
                let [caminho, compositor, titulo, instrumentos, partes, pautas] = c[..] else { return None };
                Some(Obra::nova(caminho.into(), compositor.into(), titulo.into(), instrumentos.into(), partes.parse().ok()?, pautas.parse().ok()?))
            })
            .collect();
        Some(Acervo { raiz: raiz.into(), obras })
    }

    pub fn caminho(&self, o: &Obra) -> PathBuf {
        self.raiz.join(&o.caminho)
    }

    /// Índices das obras que casam com a consulta, das mais relevantes para
    /// as menos (sem consulta, em ordem de compositor e título).
    pub fn buscar(&self, consulta: &str, so_piano: bool) -> Vec<usize> {
        let palavras: Vec<String> = normalizar(consulta).split_whitespace().map(String::from).collect();
        let mut achadas: Vec<(u32, usize)> = self
            .obras
            .iter()
            .enumerate()
            .filter(|(_, o)| !so_piano || o.piano())
            .filter_map(|(i, o)| pontuar(&o.chave, &palavras).map(|p| (p, i)))
            .collect();
        // empate: piano antes (é um piano), depois em ordem
        let obras = &self.obras;
        achadas.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| obras[b.1].piano().cmp(&obras[a.1].piano()))
                .then_with(|| obras[a.1].chave.cmp(&obras[b.1].chave))
                .then(a.1.cmp(&b.1))
        });
        achadas.into_iter().map(|(_, i)| i).collect()
    }

    /// Para começar, sem busca: as obras para piano dos compositores de
    /// [`COMPOSITORES`], alternando os compositores (cada um em ordem de
    /// título).
    pub fn destaques(&self) -> Vec<usize> {
        let nomes = nomes_dos_compositores();
        let mut grupos: Vec<Vec<usize>> = vec![Vec::new(); nomes.len()];
        for (i, o) in self.obras.iter().enumerate().filter(|(_, o)| o.piano()) {
            let compositor = o.chave.split('|').next().unwrap_or("");
            if let Some(g) = nomes.iter().position(|n| e_de(compositor, n)) {
                grupos[g].push(i);
            }
        }
        // pelo título a partir da primeira letra ("12 Etudes" entre os E)
        let titulo = |i: usize| self.obras[i].chave.split('|').nth(1).unwrap_or("").trim_start_matches(|c: char| !c.is_alphabetic());
        for g in &mut grupos {
            g.sort_by_key(|&i| titulo(i));
        }
        let maior = grupos.iter().map(Vec::len).max().unwrap_or(0);
        (0..maior).flat_map(|k| grupos.iter().filter_map(move |g| g.get(k).copied())).collect()
    }

    /// Quantas obras cada compositor de [`COMPOSITORES`] tem no acervo, dos
    /// que têm mais para os que têm menos (sem os ausentes): sugestões de
    /// busca.
    pub fn compositores(&self) -> Vec<(&'static str, usize)> {
        let nomes = nomes_dos_compositores();
        let mut contagem = vec![0; COMPOSITORES.len()];
        for o in &self.obras {
            let compositor = o.chave.split('|').next().unwrap_or("");
            for (c, n) in contagem.iter_mut().zip(&nomes) {
                if e_de(compositor, n) {
                    *c += 1;
                }
            }
        }
        let mut lista: Vec<(&str, usize)> = COMPOSITORES.iter().copied().zip(contagem).filter(|(_, c)| *c > 0).collect();
        lista.sort_by_key(|&(_, c)| std::cmp::Reverse(c));
        lista
    }
}

/// Compositores sugeridos na busca (os do repertório de piano, e os
/// brasileiros).
pub const COMPOSITORES: [&str; 32] = [
    "Bach", "Mozart", "Beethoven", "Chopin", "Debussy", "Satie", "Schubert", "Schumann", "Liszt", "Brahms", "Tchaikovsky", "Handel",
    "Haydn", "Vivaldi", "Grieg", "Mendelssohn", "Rachmaninoff", "Ravel", "Scarlatti", "Clementi", "Czerny", "Burgmüller", "Joplin",
    "Gershwin", "Villa-Lobos", "Nazareth", "Chiquinha Gonzaga", "Jobim", "Pixinguinha", "Mussorgsky", "Dvořák", "Albéniz",
];

/// [`COMPOSITORES`] normalizados, em palavras.
fn nomes_dos_compositores() -> Vec<Vec<String>> {
    COMPOSITORES.iter().map(|c| normalizar(c).split(' ').map(String::from).collect()).collect()
}

/// O compositor (normalizado) tem todas as palavras do nome (no começo de
/// uma palavra dele: "bach" em "j s bach", não em "offenbach").
fn e_de(compositor: &str, nome: &[String]) -> bool {
    nome.iter().all(|p| compositor.split(' ').any(|w| w.starts_with(p.as_str())))
}

/// Prioridade mínima para a thread atual: ela só usa o processador que
/// sobra. No Linux o "nice" vale por thread; nos outros sistemas fica como
/// está (lá ele valeria para o processo inteiro, inclusive o áudio).
fn prioridade_baixa() {
    #[cfg(target_os = "linux")]
    // SAFETY: muda só a prioridade desta thread
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 19);
    }
}

/// Pontos de uma obra para as palavras (None se alguma palavra falta).
/// Palavra inteira no compositor vale 5, no título 4; começo de palavra
/// num dos dois vale 3; no meio ou nos instrumentos, 1. Assim as obras de
/// Chopin vêm antes das que só citam Chopin no título.
fn pontuar(chave: &str, palavras: &[String]) -> Option<u32> {
    let mut campos = chave.split('|');
    let (compositor, titulo, instrumentos) = (campos.next()?, campos.next()?, campos.next().unwrap_or(""));
    let inteira = |campo: &str, p: &str| campo.split(' ').any(|w| w == p);
    let comeco = |campo: &str, p: &str| campo.split(' ').any(|w| w.starts_with(p));
    palavras.iter().try_fold(0, |soma, p| {
        let pontos = if inteira(compositor, p) {
            5
        } else if inteira(titulo, p) {
            4
        } else if comeco(titulo, p) || comeco(compositor, p) {
            3
        } else if titulo.contains(p.as_str()) || compositor.contains(p.as_str()) || instrumentos.contains(p.as_str()) {
            1
        } else {
            return None;
        };
        Some(soma + pontos)
    })
}

/// Minúsculas sem acento; o que não é letra nem número vira espaço (assim
/// "J.S. Bach" e "j s bach" se encontram). A vírgula fica, para separar os
/// instrumentos.
pub fn normalizar(s: &str) -> String {
    let mut saida = String::with_capacity(s.len());
    let mut espaco = true;
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => 'a',
            'é' | 'è' | 'ê' | 'ë' | 'ě' | 'ē' => 'e',
            'í' | 'ì' | 'î' | 'ï' | 'ī' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => 'o',
            'ú' | 'ù' | 'û' | 'ü' | 'ů' | 'ū' | 'ű' => 'u',
            'ç' | 'č' | 'ć' => 'c',
            'ñ' | 'ň' | 'ń' => 'n',
            'š' | 'ś' | 'ş' => 's',
            'ž' | 'ź' | 'ż' => 'z',
            'ř' => 'r',
            'ý' | 'ÿ' => 'y',
            'ł' => 'l',
            'đ' | 'ď' => 'd',
            'ť' => 't',
            c => c,
        };
        if c.is_alphanumeric() || c == ',' {
            saida.push(c);
            espaco = false;
        } else if !espaco {
            saida.push(' ');
            espaco = true;
        }
    }
    saida.trim_end().to_string()
}

/// Todos os MusicXML abaixo da pasta (caminhos relativos a ela).
fn listar(pasta: &Path, saida: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut pendentes = vec![pasta.to_path_buf()];
    while let Some(atual) = pendentes.pop() {
        for entrada in std::fs::read_dir(&atual)?.flatten() {
            let caminho = entrada.path();
            let Ok(tipo) = entrada.file_type() else { continue };
            if tipo.is_dir() {
                pendentes.push(caminho);
            } else if matches!(caminho.extension().and_then(|e| e.to_str()), Some("mxl" | "musicxml" | "xml"))
                && let Ok(relativo) = caminho.strip_prefix(pasta)
            {
                saida.push(relativo.to_path_buf());
            }
        }
    }
    Ok(())
}

/// O cabeçalho de um arquivo do acervo (None se não for MusicXML legível).
fn ler_obra(raiz: &Path, relativo: &Path) -> Option<Obra> {
    let texto = musicxml::ler_inicio(&raiz.join(relativo), CABECALHO).ok()?;
    if !texto.contains("<score-partwise") && !texto.contains("<score-timewise") {
        return None;
    }
    let titulo = [conteudo(&texto, "work-title"), conteudo(&texto, "movement-title"), conteudo(&texto, "credit-words")]
        .into_iter()
        .flatten()
        .find(|t| !t.is_empty())
        .unwrap_or_else(|| relativo.file_stem().map_or(String::new(), |n| n.to_string_lossy().into_owned()));
    let compositor = abrir_tag(&texto, "<creator type=\"composer\"").unwrap_or_default();
    let lista = texto.find("<part-list").map_or("", |i| &texto[i..texto[i..].find("</part-list>").map_or(texto.len(), |f| i + f)]);
    let mut nomes: Vec<String> = Vec::new();
    let mut partes = 0;
    for parte in lista.split("<score-part").skip(1) {
        partes += 1;
        let nome = conteudo(parte, "part-name").filter(|n| !n.is_empty() && !n.starts_with("MusicXML Part"));
        if let Some(n) = nome.or_else(|| conteudo(parte, "instrument-name"))
            && !n.is_empty()
            && !nomes.contains(&n)
        {
            nomes.push(n);
        }
    }
    let pautas = conteudo(&texto, "staves").and_then(|n| n.parse().ok()).unwrap_or(1);
    Some(Obra::nova(relativo.to_path_buf(), compositor, titulo, nomes.join(", "), partes, pautas))
}

/// O texto do primeiro elemento `<nome ...>texto</nome>`.
fn conteudo(xml: &str, nome: &str) -> Option<String> {
    let mut resto = xml;
    loop {
        let i = resto.find(&format!("<{nome}"))?;
        resto = &resto[i + nome.len() + 1..];
        // o nome inteiro (não "<staves-x"), e não vazio ("<x/>")
        if resto.starts_with(['>', ' ', '\t', '\n', '\r']) {
            let fim_tag = resto.find('>')?;
            if resto[..fim_tag].ends_with('/') {
                continue;
            }
            let corpo = &resto[fim_tag + 1..];
            return Some(limpar(&corpo[..corpo.find(&format!("</{nome}"))?]));
        }
    }
}

/// O texto depois de uma abertura de tag dada por inteiro (com atributos).
fn abrir_tag(xml: &str, abertura: &str) -> Option<String> {
    let corpo = &xml[xml.find(abertura)? + abertura.len()..];
    let corpo = &corpo[corpo.find('>')? + 1..];
    Some(limpar(&corpo[..corpo.find('<')?]))
}

/// Entidades XML desfeitas (duas vezes: há arquivos com "&amp;amp;") e
/// espaços (inclusive quebras e tabulações, que separam os campos do
/// índice) juntados num só.
fn limpar(s: &str) -> String {
    let uma = entidades(s);
    let duas = if uma.contains('&') { entidades(&uma) } else { uma };
    duas.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn entidades(s: &str) -> String {
    let mut saida = String::new();
    let mut resto = s;
    while let Some(i) = resto.find('&') {
        saida += &resto[..i];
        resto = &resto[i..];
        let fim = resto.find(';').filter(|&f| f <= 10);
        let entidade = fim.map(|f| &resto[1..f]);
        let c = match entidade {
            Some("amp") => Some('&'),
            Some("lt") => Some('<'),
            Some("gt") => Some('>'),
            Some("quot") => Some('"'),
            Some("apos") => Some('\''),
            Some(e) if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            Some(e) if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match (c, fim) {
            (Some(c), Some(f)) => {
                saida.push(c);
                resto = &resto[f + 1..];
            }
            _ => {
                saida.push('&');
                resto = &resto[1..];
            }
        }
    }
    saida += resto;
    saida
}

fn pasta_do_usuario(variavel: &str, padrao: &str) -> PathBuf {
    std::env::var_os(variavel)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(padrao)))
        .unwrap_or_else(std::env::temp_dir)
        .join("cancao")
}

/// Onde fica o índice de uma pasta (no cache do usuário, um por pasta).
fn arquivo_de_indice(raiz: &Path) -> PathBuf {
    // FNV-1a do caminho: nome estável e curto
    let marca = raiz.to_string_lossy().bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    pasta_do_usuario("XDG_CACHE_HOME", ".cache").join(format!("acervo-{marca:016x}.tsv"))
}

/// Preferências guardadas entre sessões: a pasta do acervo e as partituras
/// abertas por último (uma "chave=valor" por linha).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub pasta: Option<PathBuf>,
    pub recentes: Vec<PathBuf>,
}

impl Config {
    fn arquivo() -> PathBuf {
        pasta_do_usuario("XDG_CONFIG_HOME", ".config").join("cancao.conf")
    }

    pub fn carregar() -> Config {
        std::fs::read_to_string(Config::arquivo()).map(|t| Config::de_texto(&t)).unwrap_or_default()
    }

    pub fn salvar(&self) -> Result<(), String> {
        let arquivo = Config::arquivo();
        if let Some(pasta) = arquivo.parent() {
            std::fs::create_dir_all(pasta).map_err(|e| e.to_string())?;
        }
        std::fs::write(&arquivo, self.para_texto()).map_err(|e| format!("{}: {e}", arquivo.display()))
    }

    fn de_texto(texto: &str) -> Config {
        let mut c = Config::default();
        for (chave, valor) in texto.lines().filter_map(|l| l.split_once('=')) {
            match chave.trim() {
                "acervo" => c.pasta = Some(valor.trim().into()),
                "recente" => c.recentes.push(valor.trim().into()),
                _ => {}
            }
        }
        c
    }

    fn para_texto(&self) -> String {
        let mut s = String::new();
        if let Some(p) = &self.pasta {
            s += &format!("acervo={}\n", p.display());
        }
        for r in &self.recentes {
            s += &format!("recente={}\n", r.display());
        }
        s
    }

    /// Põe a partitura no começo dos recentes.
    pub fn abriu(&mut self, caminho: &Path) {
        self.recentes.retain(|r| r != caminho);
        self.recentes.insert(0, caminho.to_path_buf());
        self.recentes.truncate(RECENTES);
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    const CABECALHO_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
  <work><work-title>Pr&#228;ludium e-Moll &amp;amp; Fuge</work-title></work>
  <identification><creator type="composer">J.S. Bach</creator></identification>
  <part-list>
    <score-part id="P1"><part-name>Piano</part-name></score-part>
  </part-list>
  <part id="P1"><measure number="1"><attributes><staves>2</staves></attributes></measure></part>
</score-partwise>"#;

    fn obra(compositor: &str, titulo: &str, instrumentos: &str, partes: u32, pautas: u32) -> Obra {
        Obra::nova(format!("{titulo}.mxl").into(), compositor.into(), titulo.into(), instrumentos.into(), partes, pautas)
    }

    #[test]
    fn normaliza_sem_acento_nem_pontuacao() {
        assert_eq!(normalizar("  Präludium, J.S. BACH (1685-1750) "), "praludium, j s bach 1685 1750");
        assert_eq!(normalizar("Villa-Lobos"), "villa lobos");
        assert_eq!(normalizar("Dvořák"), "dvorak");
    }

    #[test]
    fn le_o_cabecalho_de_um_mxl() {
        let dir = std::env::temp_dir().join(format!("cancao-acervo-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("3/7")).unwrap();
        let mut z = zip::ZipWriter::new(std::fs::File::create(dir.join("3/7/Qmabc.mxl")).unwrap());
        let op = zip::write::SimpleFileOptions::default();
        z.start_file("META-INF/container.xml", op).unwrap();
        std::io::Write::write_all(&mut z, br#"<container><rootfiles><rootfile full-path="x.xml"/></rootfiles></container>"#).unwrap();
        z.start_file("x.xml", op).unwrap();
        std::io::Write::write_all(&mut z, CABECALHO_XML.as_bytes()).unwrap();
        z.finish().unwrap();
        std::fs::write(dir.join("leiame.txt"), "não é partitura").unwrap();

        let a = Acervo::indexar(&dir, &Progresso::default()).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(a.obras.len(), 1);
        let o = &a.obras[0];
        assert_eq!((o.titulo.as_str(), o.compositor.as_str(), o.instrumentos.as_str()), ("Präludium e-Moll & Fuge", "J.S. Bach", "Piano"));
        assert_eq!((o.partes, o.pautas), (1, 2));
        assert_eq!(o.caminho, Path::new("3/7/Qmabc.mxl"));
        assert!(o.piano());
        // o índice em texto volta igual
        assert_eq!(Acervo::de_texto(&a.para_texto()).unwrap().obras, a.obras);
    }

    #[test]
    fn busca_por_palavras_sem_acento_e_relevancia() {
        let a = Acervo {
            raiz: "/acervo".into(),
            obras: vec![
                obra("Heitor Villa-Lobos", "Bachianas Brasileiras No. 4", "Piano", 1, 2),
                obra("J. S. Bach", "Prelude in C", "Piano", 1, 2),
                obra("J. S. Bach", "Air", "Violin, Cello", 2, 1),
                obra("Offenbach", "Can-can", "Flute", 1, 1),
                obra("Chopin", "Nocturne Op. 9 No. 2", "Piano", 1, 2),
            ],
        };
        let titulos = |r: Vec<usize>| r.into_iter().map(|i| a.obras[i].titulo.clone()).collect::<Vec<_>>();
        // o compositor antes do título, piano antes, meio de palavra no fim
        let bach = titulos(a.buscar("bach", false));
        assert_eq!(bach, ["Prelude in C", "Air", "Bachianas Brasileiras No. 4", "Can-can"]);
        // todas as palavras precisam estar; acento e caixa não importam
        assert_eq!(titulos(a.buscar("VILLA lóbos", false)), ["Bachianas Brasileiras No. 4"]);
        assert_eq!(titulos(a.buscar("bach violin", false)), ["Air"]);
        assert_eq!(titulos(a.buscar("bach", true)).len(), 2);
        assert!(a.buscar("mozart", false).is_empty());
        // sem consulta: todas, por compositor
        assert_eq!(a.buscar("", false).len(), 5);
        assert_eq!(a.compositores(), [("Bach", 2), ("Chopin", 1), ("Villa-Lobos", 1)]);
        // sem busca: piano, alternando os compositores
        assert_eq!(titulos(a.destaques()), ["Prelude in C", "Nocturne Op. 9 No. 2", "Bachianas Brasileiras No. 4"]);
    }

    #[test]
    fn config_guarda_pasta_e_recentes() {
        let mut c = Config { pasta: Some("/home/x/mxl".into()), recentes: vec![] };
        for i in 0..20 {
            c.abriu(Path::new(&format!("/p/{i}.mxl")));
        }
        c.abriu(Path::new("/p/5.mxl"));
        assert_eq!(c.recentes.len(), RECENTES);
        assert_eq!(c.recentes[0], Path::new("/p/5.mxl"));
        assert_eq!(c.recentes.iter().filter(|r| r.ends_with("5.mxl")).count(), 1);
        assert_eq!(Config::de_texto(&c.para_texto()), c);
    }
}
