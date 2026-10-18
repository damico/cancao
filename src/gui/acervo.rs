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

//! A tela Acervo: carrega (ou indexa) a pasta configurada sem travar a
//! janela, busca a cada tecla e lembra as partituras abertas.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, mpsc};

use slint::{ModelRc, SharedString, VecModel};

use super::{ObraVis, PianoRoll, SugestaoVis};
use crate::acervo::{Acervo, Config, Obra, Progresso};

/// Linhas entregues à lista de uma vez (a busca acha todas; a lista mostra
/// as primeiras).
const MOSTRAR: usize = 300;
/// Compositores sugeridos.
const SUGESTOES: usize = 24;

enum Carga {
    Nenhuma,
    Lendo { progresso: Arc<Progresso>, fim: mpsc::Receiver<Result<Acervo, String>> },
    Pronta(Acervo),
    Erro(String),
}

pub struct TelaAcervo {
    config: Config,
    /// guardar a configuração (a captura em PNG não mexe na do usuário)
    guardar: bool,
    carga: Carga,
    /// o que a lista mostra: posições no acervo
    lista: Vec<usize>,
}

impl TelaAcervo {
    pub fn new() -> TelaAcervo {
        TelaAcervo { config: Config::carregar(), guardar: true, carga: Carga::Nenhuma, lista: Vec::new() }
    }

    /// Um acervo já lido, sem configuração guardada (para a captura).
    pub fn pronta(acervo: Acervo) -> TelaAcervo {
        let config = Config { pasta: Some(acervo.raiz.clone()), recentes: Vec::new() };
        TelaAcervo { config, guardar: false, carga: Carga::Pronta(acervo), lista: Vec::new() }
    }

    /// Mostra a tela; na primeira vez, começa a ler a pasta configurada.
    pub fn mostrar(&mut self, ui: &PianoRoll) {
        if matches!(self.carga, Carga::Nenhuma) && self.config.pasta.is_some() {
            self.carregar(false);
        }
        ui.set_tela(1);
        self.entregar(ui);
        self.buscar(ui);
    }

    /// Nova pasta: fica guardada e é lida (ou indexada).
    pub fn trocar_pasta(&mut self, ui: &PianoRoll, pasta: PathBuf) {
        self.config.pasta = Some(pasta);
        self.salvar();
        self.carregar(false);
        self.entregar(ui);
    }

    pub fn reindexar(&mut self, ui: &PianoRoll) {
        self.carregar(true);
        self.entregar(ui);
    }

    fn salvar(&mut self) {
        if self.guardar
            && let Err(e) = self.config.salvar()
        {
            eprintln!("configuração não salva: {e}");
        }
    }

    fn carregar(&mut self, reindexar: bool) {
        let Some(pasta) = self.config.pasta.clone() else { return };
        let progresso = Arc::new(Progresso::default());
        let (envia, fim) = mpsc::channel();
        let p = progresso.clone();
        std::thread::spawn(move || {
            let _ = envia.send(Acervo::abrir(&pasta, &p, reindexar));
        });
        self.carga = Carga::Lendo { progresso, fim };
    }

    /// A cada quadro: andamento da leitura e, quando ela acaba, a busca.
    pub fn atualizar(&mut self, ui: &PianoRoll) {
        let Carga::Lendo { progresso, fim } = &self.carga else { return };
        match fim.try_recv() {
            Ok(Ok(a)) => self.carga = Carga::Pronta(a),
            Ok(Err(e)) => self.carga = Carga::Erro(e),
            Err(mpsc::TryRecvError::Empty) => {
                let (feitos, total) = (progresso.feitos.load(Relaxed), progresso.total.load(Relaxed));
                if total > 0 {
                    ui.set_acervo_progresso(feitos as f32 / total as f32);
                    ui.set_acervo_status(format!("Lendo {} de {} arquivos (só na primeira vez)", milhares(feitos), milhares(total)).into());
                }
                return;
            }
            Err(mpsc::TryRecvError::Disconnected) => self.carga = Carga::Erro("a leitura do acervo parou".into()),
        }
        self.entregar(ui);
        self.buscar(ui);
    }

    /// Estado, pasta e sugestões.
    fn entregar(&self, ui: &PianoRoll) {
        ui.set_acervo_pasta(self.config.pasta.as_ref().map_or(String::new(), |p| p.display().to_string()).into());
        let (estado, status) = match &self.carga {
            Carga::Nenhuma => (0, String::new()),
            Carga::Lendo { .. } => (1, "Abrindo o índice...".into()),
            Carga::Pronta(a) => (2, format!("{} partituras", milhares(a.obras.len()))),
            Carga::Erro(e) => (3, format!("Não foi possível ler o acervo: {e}")),
        };
        ui.set_acervo_estado(estado);
        ui.set_acervo_status(status.into());
        ui.set_acervo_progresso(0.0);
        let sugestoes: Vec<SugestaoVis> = match &self.carga {
            Carga::Pronta(a) => a
                .compositores()
                .into_iter()
                .take(SUGESTOES)
                .map(|(nome, c)| SugestaoVis { nome: nome.into(), contagem: c as i32 })
                .collect(),
            _ => Vec::new(),
        };
        ui.set_acervo_sugestoes(ModelRc::new(VecModel::from(sugestoes)));
    }

    /// Refaz a lista com a consulta e o filtro da tela. Sem consulta, as
    /// abertas por último (ou todas, se ainda não há nenhuma).
    pub fn buscar(&mut self, ui: &PianoRoll) {
        let Carga::Pronta(a) = &self.carga else { return };
        let consulta = ui.get_acervo_consulta().to_string();
        let so_piano = ui.get_acervo_so_piano();
        let recentes: Vec<usize> = if consulta.trim().is_empty() {
            self.config
                .recentes
                .iter()
                .filter_map(|r| r.strip_prefix(&a.raiz).ok())
                .filter_map(|r| a.obras.iter().position(|o| o.caminho == r))
                .filter(|&i| !so_piano || a.obras[i].piano())
                .collect()
        } else {
            Vec::new()
        };
        let (lista, titulo) = if !recentes.is_empty() {
            (recentes, "Abertas recentemente (digite para procurar entre todas)".to_string())
        } else {
            let achadas = if consulta.trim().is_empty() { a.destaques() } else { a.buscar(&consulta, so_piano) };
            let n = achadas.len();
            let titulo = match (consulta.trim().is_empty(), n) {
                (_, 0) => String::new(),
                (true, _) => format!("Para começar: {} obras para piano dos compositores acima; digite para procurar entre todas", milhares(n)),
                (false, 1) => "1 encontrada".into(),
                (false, n) if n > MOSTRAR => format!("{} encontradas (as {MOSTRAR} mais relevantes; mais palavras refinam)", milhares(n)),
                (false, n) => format!("{} encontradas", milhares(n)),
            };
            (achadas, titulo)
        };
        let linhas: Vec<ObraVis> = lista.iter().take(MOSTRAR).map(|&i| obra_vis(a, i, &a.obras[i])).collect();
        self.lista = lista;
        ui.set_acervo_titulo_lista(titulo.into());
        ui.set_acervo_resultados(ModelRc::new(VecModel::from(linhas)));
        let n = self.lista.len().min(MOSTRAR) as i32;
        if ui.get_acervo_selecionada() >= n {
            ui.set_acervo_selecionada(0);
        }
    }

    /// O arquivo de uma obra do acervo (a posição vem de [`ObraVis::indice`]).
    pub fn caminho(&self, indice: usize) -> Option<PathBuf> {
        let Carga::Pronta(a) = &self.carga else { return None };
        a.obras.get(indice).map(|o| a.caminho(o))
    }

    /// A partitura foi aberta: vai para o começo das recentes.
    pub fn abriu(&mut self, caminho: &Path) {
        self.config.abriu(caminho);
        self.salvar();
    }
}

fn obra_vis(a: &Acervo, indice: usize, o: &Obra) -> ObraVis {
    let formacao = match (o.partes, o.pautas) {
        (0 | 1, 2) => "1 parte, 2 pautas".to_string(),
        (0 | 1, _) => "1 parte".to_string(),
        (n, _) => format!("{n} partes"),
    };
    ObraVis {
        indice: indice as i32,
        titulo: SharedString::from(o.titulo.as_str()),
        compositor: SharedString::from(o.compositor.as_str()),
        instrumentos: SharedString::from(o.instrumentos.as_str()),
        formacao: formacao.into(),
        piano: o.piano(),
        arquivo: a.caminho(o).display().to_string().into(),
    }
}

/// 254035 para "254.035".
fn milhares(n: usize) -> String {
    let s = n.to_string();
    let mut saida = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            saida.push('.');
        }
        saida.push(c);
    }
    saida
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn milhares_com_ponto() {
        assert_eq!(milhares(7), "7");
        assert_eq!(milhares(1000), "1.000");
        assert_eq!(milhares(254035), "254.035");
        assert_eq!(milhares(1234567), "1.234.567");
    }
}
