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

//! Painel da partitura quando o programa é compilado sem a feature
//! "partitura" (sem o Verovio): mostra só um aviso.

use super::PianoRoll;
use crate::partitura::Partitura;

pub struct Painel;

impl Painel {
    pub fn new(_fonte: Option<String>, _partitura: Partitura) -> Self {
        Painel
    }
    pub fn zoom(&mut self, _passo: i32) {}
    pub fn rolar(&mut self, _dy: f64) {}
    pub fn mostrar(&mut self, _indice: Option<usize>) {}
    pub fn nota_da_marca(&self, _id: usize) -> Option<(Option<usize>, f64)> {
        None
    }
    pub fn atualizar(&mut self, ui: &PianoRoll, _agora: Option<f64>, _tocando: bool, _selecionada: Option<usize>) {
        ui.set_status_partitura("Compilado sem a feature \"partitura\" (Verovio).".into());
    }
    pub fn diagramar_agora(&mut self, _largura: u32) {}
}
