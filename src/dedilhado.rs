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

//! Dedilhado: qual dedo toca cada nota, para uma mão humana.
//!
//! Modelo ergonômico de Parncutt, Sloboda, Clarke, Raekallio e Desain (1997),
//! "An ergonomic model of keyboard fingering for melodic fragments": para cada
//! par de dedos há uma abertura relaxada, uma confortável e uma praticável (em
//! semitons, mão adulta média), e regras somam pontos de dificuldade
//! (abertura além do confortável, dedo fraco, polegar em tecla preta,
//! passagem do polegar, mudanças de posição da mão...). A sequência de dedos
//! de menor custo total sai por programação dinâmica de segunda ordem, porque
//! algumas regras olham grupos de três notas.
//!
//! Dedos: 1 = polegar ... 5 = mínimo. A mão esquerda é o espelho da direita:
//! as distâncias são calculadas com as alturas invertidas.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mao {
    Esquerda,
    Direita,
}

impl Mao {
    pub fn sigla(self) -> &'static str {
        match self {
            Mao::Esquerda => "ME",
            Mao::Direita => "MD",
        }
    }
}

/// Limites de abertura (semitons) de um par de dedos (i < j) da mão direita,
/// do dedo i para o dedo j: praticável, confortável e relaxado.
#[derive(Clone, Copy)]
struct Limites {
    min_prat: i32,
    min_conf: i32,
    min_rel: i32,
    max_rel: i32,
    max_conf: i32,
    max_prat: i32,
}

const fn lim(min_prat: i32, min_conf: i32, min_rel: i32, max_rel: i32, max_conf: i32, max_prat: i32) -> Limites {
    Limites { min_prat, min_conf, min_rel, max_rel, max_conf, max_prat }
}

/// Tabela 1 de Parncutt et al. (1997), mão direita.
fn limites(i: u8, j: u8) -> Limites {
    match (i, j) {
        (1, 2) => lim(-5, -3, 1, 5, 8, 10),
        (1, 3) => lim(-4, -2, 3, 7, 10, 12),
        (1, 4) => lim(-3, -1, 5, 9, 12, 14),
        (1, 5) => lim(-1, 1, 7, 10, 13, 15),
        (2, 3) => lim(1, 1, 1, 2, 3, 5),
        (2, 4) => lim(1, 1, 3, 4, 5, 7),
        (2, 5) => lim(2, 2, 5, 6, 8, 10),
        (3, 4) => lim(1, 1, 1, 2, 2, 4),
        (3, 5) => lim(1, 1, 3, 4, 5, 7),
        (4, 5) => lim(1, 1, 1, 2, 3, 5),
        _ => unreachable!("par de dedos inválido {i}-{j}"),
    }
}

/// Tecla preta? (tecla 0 = A0)
pub fn preta(tecla: u8) -> bool {
    matches!((tecla as i32 + 9) % 12, 1 | 3 | 6 | 8 | 10)
}

/// Uma nota na mão: altura espelhada (para a mão esquerda) e cor da tecla.
#[derive(Clone, Copy)]
struct Toque {
    altura: i32,
    preta: bool,
}

fn toque(mao: Mao, tecla: u8) -> Toque {
    let t = tecla as i32;
    Toque { altura: if mao == Mao::Direita { t } else { -t }, preta: preta(tecla) }
}

/// Distância do par ordenado (dedo menor para dedo maior) e seus limites.
fn par(fa: u8, a: Toque, fb: u8, b: Toque) -> (i32, Limites) {
    if fa < fb { (b.altura - a.altura, limites(fa, fb)) } else { (a.altura - b.altura, limites(fb, fa)) }
}

/// Regras de abertura (1, 2 e 3 de Parncutt): além do confortável e fora do
/// relaxado.
fn custo_abertura(fa: u8, a: Toque, fb: u8, b: Toque) -> f32 {
    let (d, l) = par(fa, a, fb, b);
    let polegar = fa == 1 || fb == 1;
    let peso_span = if polegar { 1.0 } else { 2.0 };
    let mut c = 0.0;
    // 1: estiramento
    if d > l.max_conf {
        c += 2.0 * (d - l.max_conf) as f32;
    } else if d < l.min_conf {
        c += 2.0 * (l.min_conf - d) as f32;
    }
    // 2 e 3: abertura menor ou maior que a relaxada
    if d < l.min_rel {
        c += peso_span * (l.min_rel - d) as f32;
    } else if d > l.max_rel {
        c += peso_span * (d - l.max_rel) as f32;
    }
    c
}

/// Custo de tocar `b` com `fb` logo depois de `a` com `fa`. `acordes`: algum
/// dos dois momentos é um acorde.
fn custo_passo(fa: u8, a: Toque, fb: u8, b: Toque, acordes: bool) -> f32 {
    if fa == fb {
        let salto = (b.altura - a.altura).abs() as f32;
        return if salto == 0.0 {
            0.0 // repetir a tecla com o mesmo dedo
        } else if acordes {
            // entre acordes a mão inteira salta de posição (o baixo em oitavas
            // é 5-1, 5-1, 5-1): custo pequeno, proporcional à distância
            0.5 + 0.2 * salto
        } else {
            // numa melodia não dá para ligar as notas com o mesmo dedo
            // (Parncutt nem permite), então custa muito
            15.0 + salto
        };
    }
    let mut c = custo_abertura(fa, a, fb, b);
    // 8: o 3 seguido do 4
    if fa == 3 && fb == 4 {
        c += 1.0;
    }
    // 9: 3 e 4 seguidos, com o 3 na branca e o 4 na preta
    let (t3, t4) = if fa == 3 && fb == 4 { (Some(a), Some(b)) } else if fa == 4 && fb == 3 { (Some(b), Some(a)) } else { (None, None) };
    if let (Some(t3), Some(t4)) = (t3, t4) {
        if !t3.preta && t4.preta {
            c += 1.0;
        }
    }
    // 10 e 11: polegar ou mínimo na preta, vizinho de branca
    if fb == 1 && b.preta && !a.preta {
        c += 2.0;
    }
    if fa == 1 && a.preta && !b.preta {
        c += 2.0;
    }
    if fb == 5 && b.preta && !a.preta {
        c += 2.0;
    }
    if fa == 5 && a.preta && !b.preta {
        c += 2.0;
    }
    // 12: passagem do polegar (por baixo, subindo; ou dedo por cima, descendo)
    let subindo = b.altura > a.altura;
    let passagem = (fb == 1 && fa != 1 && subindo) || (fa == 1 && fb != 1 && !subindo && b.altura != a.altura);
    if passagem {
        let (baixo, dedo_baixo, alto, dedo_alto) = if a.altura < b.altura { (a, fa, b, fb) } else { (b, fb, a, fa) };
        c += if !baixo.preta && dedo_baixo != 1 && alto.preta && dedo_alto == 1 {
            3.0
        } else if baixo.preta == alto.preta {
            1.0
        } else {
            0.0
        };
    }
    c
}

/// Regras de grupos de três notas (4, 5 e 7 de Parncutt): a mão muda de
/// posição quando a 1ª e a 3ª notas ficam fora da abertura confortável dos
/// seus dedos; a mudança é completa (2 pontos) quando o polegar toca a do meio,
/// ela fica entre as outras duas e a abertura passa do praticável, e meia (1
/// ponto) nos outros casos. Soma-se o quanto a abertura passou do
/// confortável, e 1 ponto para os dedos 3, 4 e 5 seguidos em qualquer ordem.
/// O mesmo dedo na 1ª e na 3ª nota, em teclas diferentes, também é mudança
/// de posição (a tabela de Parncutt só cobre pares de dedos diferentes).
fn custo_trio(f1: u8, a: Toque, f2: u8, b: Toque, f3: u8, c: Toque) -> f32 {
    let mut custo = 0.0;
    let mut dedos = [f1, f2, f3];
    dedos.sort_unstable();
    if dedos == [3, 4, 5] {
        custo += 1.0;
    }
    if f1 == f3 {
        // o mesmo dedo em teclas diferentes, com uma nota no meio: a mão
        // inteira se deslocou
        let desloca = (c.altura - a.altura).abs();
        return if desloca > 0 { custo + 1.0 + desloca as f32 } else { custo };
    }
    let (d, l) = par(f1, a, f3, c);
    if d > l.max_conf || d < l.min_conf {
        let entre = (a.altura < b.altura && b.altura < c.altura) || (c.altura < b.altura && b.altura < a.altura);
        let completa = f2 == 1 && entre && (d > l.max_prat || d < l.min_prat);
        custo += if completa { 2.0 } else { 1.0 };
        custo += if d > l.max_conf { (d - l.max_conf) as f32 } else { (l.min_conf - d) as f32 };
    }
    custo
}

/// Custo fixo de uma posição: dedos fracos (regra 6), polegar na preta
/// (regra 10) e, num acorde, a abertura entre dedos vizinhos. Infinito se
/// algum par passar do praticável.
///
/// Regra extra para acordes (o modelo de Parncutt é para melodias, e nele
/// 1-2-4 e 1-3-5 empatam numa tríade): o anelar é o dedo menos independente,
/// ligado ao médio e ao mínimo por conexões de tendão (juncturae tendinum),
/// e sustentá-lo num acorde custa um pouco mais.
fn custo_posicao(dedos: &[u8], toques: &[Toque]) -> f32 {
    let mut c = 0.0;
    for (&d, t) in dedos.iter().zip(toques) {
        if d >= 4 {
            c += 1.0;
        }
        if d == 4 && dedos.len() > 1 {
            c += 0.5;
        }
        if d == 1 && t.preta {
            c += 1.0;
        }
    }
    for i in 0..dedos.len() {
        for j in i + 1..dedos.len() {
            let (d, l) = par(dedos[i], toques[i], dedos[j], toques[j]);
            if d < l.min_prat || d > l.max_prat {
                return f32::INFINITY;
            }
        }
        if i + 1 < dedos.len() {
            c += custo_abertura(dedos[i], toques[i], dedos[i + 1], toques[i + 1]);
        }
    }
    c
}

/// Todas as escolhas de `k` dedos em ordem crescente.
fn combinacoes(k: usize) -> Vec<Vec<u8>> {
    let mut saida = Vec::new();
    let mut atual = Vec::new();
    fn rec(inicio: u8, k: usize, atual: &mut Vec<u8>, saida: &mut Vec<Vec<u8>>) {
        if atual.len() == k {
            saida.push(atual.clone());
            return;
        }
        for d in inicio..=5 {
            atual.push(d);
            rec(d + 1, k, atual, saida);
            atual.pop();
        }
    }
    rec(1, k, &mut atual, &mut saida);
    saida
}

/// Dedilhado de um momento (notas abaixadas juntas por uma mão).
#[derive(Debug, Clone, PartialEq)]
pub struct Posicao {
    /// teclas em ordem crescente
    pub teclas: Vec<u8>,
    /// dedo de cada tecla
    pub dedos: Vec<u8>,
    /// false: o acorde não cabe na mão (o player arpeja)
    pub cabe: bool,
}

/// Escolhe os dedos de uma sequência de momentos de uma mão. Cada momento
/// tem de 1 a 5 teclas.
pub fn dedilhar(mao: Mao, momentos: &[Vec<u8>]) -> Vec<Posicao> {
    // estados possíveis de cada momento: dedos em ordem com as alturas
    // espelhadas (na mão esquerda o polegar fica na nota mais aguda)
    let preparados: Vec<(Vec<u8>, Vec<Toque>, Vec<Vec<u8>>)> = momentos
        .iter()
        .map(|m| {
            let mut teclas = m.clone();
            teclas.sort_unstable();
            teclas.dedup();
            let mut toques: Vec<Toque> = teclas.iter().map(|&t| toque(mao, t)).collect();
            toques.sort_by_key(|t| t.altura);
            // a mesma tecla em duas vozes é abaixada uma vez só: os dedos são
            // escolhidos para as teclas distintas
            let k = teclas.len().clamp(1, 5);
            (teclas, toques, combinacoes(k))
        })
        .collect();

    // custo de um estado: posição + (se não cabe) penalidade grande, para o
    // acorde ainda receber dedos e ser arpejado
    let custo_estado = |toques: &[Toque], dedos: &[u8]| {
        let c = custo_posicao(dedos, toques);
        if c.is_finite() { c } else { 1000.0 }
    };
    // as regras de pares e de trios valem nas linhas da nota mais grave e da
    // mais aguda de cada momento (iguais numa melodia de notas soltas)
    let linhas = |toques: &[&[Toque]]| {
        let ultima = toques.iter().any(|t| t.len() > 1);
        if ultima { vec![false, true] } else { vec![false] }
    };
    let pega = |t: &[Toque], d: &[u8], aguda: bool| if aguda { (d[d.len() - 1], t[t.len() - 1]) } else { (d[0], t[0]) };
    let custo_transicao = |ta: &[Toque], da: &[u8], tb: &[Toque], db: &[u8]| {
        let acordes = ta.len() > 1 || tb.len() > 1;
        linhas(&[ta, tb])
            .into_iter()
            .map(|ag| {
                let ((fa, a), (fb, b)) = (pega(ta, da, ag), pega(tb, db, ag));
                custo_passo(fa, a, fb, b, acordes)
            })
            .sum::<f32>()
    };
    // as regras de trios valem para melodias (Parncutt as define assim): entre
    // acordes a mão salta de posição o tempo todo
    let custo_tres = |t1: &[Toque], d1: &[u8], t2: &[Toque], d2: &[u8], t3: &[Toque], d3: &[u8]| {
        if t1.len() > 1 || t2.len() > 1 || t3.len() > 1 {
            return 0.0;
        }
        linhas(&[t1, t2, t3])
            .into_iter()
            .map(|ag| {
                let ((f1, a), (f2, b), (f3, c)) = (pega(t1, d1, ag), pega(t2, d2, ag), pega(t3, d3, ag));
                custo_trio(f1, a, f2, b, f3, c)
            })
            .sum::<f32>()
    };

    let n = preparados.len();
    if n == 0 {
        return Vec::new();
    }
    let estados = |i: usize| &preparados[i].2;
    let toques = |i: usize| preparados[i].1.as_slice();

    // programação dinâmica de 2ª ordem: custo[(p, e)] = melhor custo terminando
    // com o estado p no momento anterior e e no atual
    let mut escolha = vec![0usize; n];
    if n == 1 {
        let base: Vec<f32> = estados(0).iter().map(|d| custo_estado(toques(0), d)).collect();
        escolha[0] = (0..base.len()).min_by(|&a, &b| base[a].total_cmp(&base[b])).unwrap();
    } else {
        let mut custo: Vec<Vec<f32>> = estados(0)
            .iter()
            .map(|d0| {
                estados(1)
                    .iter()
                    .map(|d1| custo_estado(toques(0), d0) + custo_estado(toques(1), d1) + custo_transicao(toques(0), d0, toques(1), d1))
                    .collect()
            })
            .collect();
        // origem[i][p][e]: estado do momento i-2 no melhor caminho
        let mut origem: Vec<Vec<Vec<usize>>> = vec![Vec::new(), Vec::new()];
        for i in 2..n {
            let (e1, e2) = (estados(i - 1), estados(i));
            let mut novo = vec![vec![f32::INFINITY; e2.len()]; e1.len()];
            let mut de = vec![vec![0usize; e2.len()]; e1.len()];
            for (p, d1) in e1.iter().enumerate() {
                for (e, d2) in e2.iter().enumerate() {
                    let local = custo_estado(toques(i), d2) + custo_transicao(toques(i - 1), d1, toques(i), d2);
                    for (q, d0) in estados(i - 2).iter().enumerate() {
                        let c = custo[q][p] + local + custo_tres(toques(i - 2), d0, toques(i - 1), d1, toques(i), d2);
                        if c < novo[p][e] {
                            novo[p][e] = c;
                            de[p][e] = q;
                        }
                    }
                }
            }
            custo = novo;
            origem.push(de);
        }
        // melhor par final e caminho de volta
        let (mut p, mut e) = (0, 0);
        for (pi, linha) in custo.iter().enumerate() {
            for (ei, &c) in linha.iter().enumerate() {
                if c < custo[p][e] {
                    (p, e) = (pi, ei);
                }
            }
        }
        escolha[n - 1] = e;
        escolha[n - 2] = p;
        for i in (2..n).rev() {
            let q = origem[i][escolha[i - 1]][escolha[i]];
            escolha[i - 2] = q;
        }
    }

    preparados
        .iter()
        .zip(&escolha)
        .map(|((teclas, toques, estados), &e)| {
            let dedos_por_altura = &estados[e];
            // de volta à ordem crescente das teclas (na esquerda a ordem inverte)
            let mut dedos = dedos_por_altura.clone();
            if mao == Mao::Esquerda {
                dedos.reverse();
            }
            Posicao {
                teclas: teclas.clone(),
                dedos,
                cabe: custo_posicao(dedos_por_altura, toques).is_finite(),
            }
        })
        .collect()
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::corda::tecla_de_nome;

    fn notas(nomes: &str) -> Vec<Vec<u8>> {
        nomes.split_whitespace().map(|n| vec![tecla_de_nome(n).unwrap() as u8]).collect()
    }

    fn dedos(p: &[Posicao]) -> Vec<u8> {
        p.iter().map(|p| p.dedos[0]).collect()
    }

    #[test]
    fn escala_de_do_mao_direita() {
        let p = dedilhar(Mao::Direita, &notas("C4 D4 E4 F4 G4 A4 B4 C5"));
        assert_eq!(dedos(&p), [1, 2, 3, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn escala_de_do_mao_esquerda() {
        let p = dedilhar(Mao::Esquerda, &notas("C3 D3 E3 F3 G3 A3 B3 C4"));
        assert_eq!(dedos(&p), [5, 4, 3, 2, 1, 3, 2, 1]);
    }

    #[test]
    fn triade_de_do() {
        let c = |n| tecla_de_nome(n).unwrap() as u8;
        let p = dedilhar(Mao::Direita, &[vec![c("C4"), c("E4"), c("G4")]]);
        assert_eq!(p[0].dedos, [1, 3, 5]);
        let p = dedilhar(Mao::Esquerda, &[vec![c("C3"), c("E3"), c("G3")]]);
        assert_eq!(p[0].dedos, [5, 3, 1]);
        assert!(p[0].cabe);
        let p = dedilhar(Mao::Direita, &[vec![c("C4"), c("E4"), c("G4"), c("C5")]]);
        assert_eq!(p[0].dedos, [1, 2, 3, 5]);
    }

    #[test]
    fn baixo_em_oitavas_e_5_1() {
        let c = |n| tecla_de_nome(n).unwrap() as u8;
        let oitavas: Vec<Vec<u8>> = ["C2", "A1", "F1", "G1", "C2"]
            .iter()
            .map(|&n| vec![c(n), c(n) + 12])
            .collect();
        for p in dedilhar(Mao::Esquerda, &oitavas) {
            assert_eq!(p.dedos, [5, 1], "{:?}", p.teclas);
        }
        let p = dedilhar(Mao::Direita, &[vec![c("F4"), c("A4"), c("C5")]]);
        assert_eq!(p[0].dedos, [1, 3, 5]);
    }

    #[test]
    fn mesma_tecla_em_duas_vozes() {
        let c = |n| tecla_de_nome(n).unwrap() as u8;
        let p = dedilhar(Mao::Direita, &[vec![c("C4"), c("E4"), c("E4")], vec![c("D4")]]);
        assert_eq!(p[0].teclas, [c("C4"), c("E4")]);
        assert_eq!(p[0].dedos.len(), 2);
    }

    #[test]
    fn acorde_grande_demais_nao_cabe() {
        // C4 a E5: 16 semitons, além do praticável entre polegar e mínimo (15)
        let c = |n| tecla_de_nome(n).unwrap() as u8;
        let p = dedilhar(Mao::Direita, &[vec![c("C4"), c("E5")]]);
        assert!(!p[0].cabe);
        let p = dedilhar(Mao::Direita, &[vec![c("C4"), c("C5")]]);
        assert!(p[0].cabe, "a oitava cabe");
    }

    #[test]
    fn teclas_pretas() {
        assert!(preta(tecla_de_nome("C#4").unwrap() as u8));
        assert!(preta(tecla_de_nome("A#0").unwrap() as u8));
        assert!(!preta(tecla_de_nome("A0").unwrap() as u8));
        assert!(!preta(tecla_de_nome("C8").unwrap() as u8));
    }
}
