use crate::{
    system::Mass, AminoAcid, LinearPeptide, MassTolerance, MolecularFormula, MultiMolecularFormula,
    SequenceElement,
};

use super::{align_type::*, piece::*, scoring::*, Alignment};
use crate::uom::num_traits::Zero;

/// Create an alignment of two peptides based on mass and homology.
/// The substitution matrix is in the exact same order as the definition of [`AminoAcid`].
/// The [`MassTolerance`] sets the tolerance for two sets of amino acids to be regarded as the same mass.
/// The [`Type`] controls the alignment behaviour, global/local or anything in between.
/// # Panics
/// It panics when the length of `seq_a` or `seq_b` is bigger than [`isize::MAX`].
/// It also panics if `STEPS > 32`, it cannot store the local scores in an i8 otherwise.
/// The peptides are assumed to be simple (see [`LinearPeptide::assume_simple`]).
#[allow(clippy::too_many_lines)]
pub fn align<const STEPS: usize>(
    seq_a: LinearPeptide,
    seq_b: LinearPeptide,
    scoring_matrix: &[[i8; AminoAcid::TOTAL_NUMBER]; AminoAcid::TOTAL_NUMBER],
    tolerance: MassTolerance,
    ty: Type,
) -> Alignment {
    // Enforce some assumptions
    let seq_a = seq_a.assume_simple();
    let seq_b = seq_b.assume_simple();
    assert!(isize::try_from(seq_a.len()).is_ok());
    assert!(isize::try_from(seq_b.len()).is_ok());
    assert!(STEPS <= 32);

    let mut matrix = Matrix::new(&seq_a, &seq_b);
    let mut high = (0, 0, 0);
    let masses_a = calculate_masses(STEPS, &seq_a);
    let masses_b = calculate_masses(STEPS, &seq_b);

    if ty.left_a() {
        matrix.global_start(true);
    }
    if ty.left_b() {
        matrix.global_start(false);
    }

    let mut values = Vec::with_capacity(STEPS * STEPS + 2);
    for index_a in 1..=seq_a.len() {
        for index_b in 1..=seq_b.len() {
            values.clear();
            for len_a in 0..=STEPS {
                for len_b in 0..=STEPS {
                    if len_a == 0 && len_b != 1
                        || len_a != 1 && len_b == 0
                        || len_a > index_a
                        || len_b > index_b
                    {
                        continue; // Do not allow double gaps, any double gaps will be counted as two gaps after each other
                    }
                    let base_score = matrix[(index_a - len_a, index_b - len_b)].score;
                    // len_a and b are always <= STEPS
                    let piece = if len_a == 0 || len_b == 0 {
                        // First check the score to be used for affine gaps
                        let prev = &matrix[(index_a - len_a, index_b - len_b)];
                        let score =
                            if prev.step_a == 0 && len_a == 0 || prev.step_b == 0 && len_b == 0 {
                                GAP_EXTEND_PENALTY
                            } else {
                                GAP_START_PENALTY
                            };
                        Some(Piece::new(
                            base_score + score as isize,
                            score,
                            MatchType::Gap,
                            len_a as u8,
                            len_b as u8,
                        ))
                    } else if len_a == 1 && len_b == 1 {
                        Some(score_pair(
                            (&seq_a.sequence[index_a - 1], &masses_a[0][index_a]),
                            (&seq_b.sequence[index_b - 1], &masses_b[0][index_b]),
                            scoring_matrix,
                            base_score,
                            tolerance,
                        ))
                    } else {
                        score(
                            (
                                &seq_a.sequence[index_a - len_a..index_a],
                                &masses_a[len_a - 1][index_a],
                            ),
                            (
                                &seq_b.sequence[index_b - len_b..index_b],
                                &masses_b[len_b - 1][index_b],
                            ),
                            base_score,
                            tolerance,
                        )
                    };
                    if let Some(p) = piece {
                        values.push(p);
                    }
                }
            }
            let value = values
                .iter()
                .max_by(|x, y| x.score.cmp(&y.score))
                .cloned()
                .unwrap_or_default();
            if value.score >= high.0 {
                high = (value.score, index_a, index_b);
            }
            matrix[(index_a, index_b)] = value;
        }
    }

    let (high_score, start_a, start_b, path) = matrix.trace_path(ty, high);

    let max_score = (seq_a.sequence
        [start_a..start_a + path.iter().map(|p| p.step_a as usize).sum::<usize>()]
        .iter()
        .map(|a| scoring_matrix[a.aminoacid as usize][a.aminoacid as usize] as isize)
        .sum::<isize>()
        + seq_b.sequence[start_b..start_b + path.iter().map(|p| p.step_b as usize).sum::<usize>()]
            .iter()
            .map(|a| scoring_matrix[a.aminoacid as usize][a.aminoacid as usize] as isize)
            .sum::<isize>())
        / 2;

    Alignment {
        absolute_score: high_score,
        normalised_score: high_score as f64 / max_score as f64,
        maximal_score: max_score,
        path,
        start_a,
        start_b,
        seq_a,
        seq_b,
        ty,
        maximal_step: STEPS,
    }
}

/// Score a pair of sequence elements (AA + mods)
fn score_pair(
    a: (&SequenceElement, &[Mass]),
    b: (&SequenceElement, &[Mass]),
    alphabet: &[[i8; AminoAcid::TOTAL_NUMBER]; AminoAcid::TOTAL_NUMBER],
    score: isize,
    tolerance: MassTolerance,
) -> Piece {
    match (a.0 == b.0, tolerance.any_within(a.1, a.1)) {
        (true, true) => {
            let local = alphabet[a.0.aminoacid as usize][b.0.aminoacid as usize];
            Piece::new(score + local as isize, local, MatchType::FullIdentity, 1, 1)
        }
        (true, false) => {
            let local =
                alphabet[a.0.aminoacid as usize][b.0.aminoacid as usize] + MASS_MISMATCH_PENALTY;
            Piece::new(
                score + local as isize,
                local,
                MatchType::IdentityMassMismatch,
                1,
                1,
            )
        }
        (false, true) => Piece::new(
            score + ISOBARIC as isize,
            ISOBARIC,
            MatchType::Isobaric,
            1,
            1,
        ),
        (false, false) => Piece::new(
            score + MISMATCH as isize,
            MISMATCH,
            MatchType::Mismatch,
            1,
            1,
        ),
    }
}

/// Score two sets of aminoacids (it will only be called when at least one of a and b has len > 1)
/// Returns none if no sensible explanation can be made
fn score(
    a: (&[SequenceElement], &[Mass]),
    b: (&[SequenceElement], &[Mass]),
    score: isize,
    tolerance: MassTolerance,
) -> Option<Piece> {
    if tolerance.any_within(a.1, b.1) {
        let mut b_copy = b.0.to_owned();
        let rotated = a.0.len() == b.0.len()
            && a.0.iter().all(|el| {
                b_copy.iter().position(|x| x == el).map_or(false, |pos| {
                    b_copy.remove(pos);
                    true
                })
            });
        #[allow(clippy::cast_possible_wrap)]
        let local = if rotated {
            BASE_SPECIAL + ROTATED * a.0.len() as i8
        } else {
            BASE_SPECIAL + ISOBARIC * (a.0.len() + b.0.len()) as i8 / 2
        };
        Some(Piece::new(
            score + local as isize,
            local,
            if rotated {
                MatchType::Rotation
            } else {
                MatchType::Isobaric
            },
            a.0.len() as u8,
            b.0.len() as u8,
        ))
    } else {
        None
    }
}

/// Get the masses of all subsets of up to the given number of steps as a lookup table.
/// The result should be is index by [steps-1][index]
fn calculate_masses(steps: usize, sequence: &LinearPeptide) -> Vec<Vec<Vec<Mass>>> {
    (1..=steps)
        .map(|size| {
            (0..=sequence.len())
                .map(|index| {
                    if index < size {
                        vec![Mass::zero()]
                    } else {
                        sequence.sequence[index - size..index]
                            .iter()
                            .map(SequenceElement::formulas_all)
                            .sum::<MultiMolecularFormula>()
                            .iter()
                            .map(MolecularFormula::monoisotopic_mass)
                            .collect()
                    }
                })
                .collect::<Vec<Vec<Mass>>>()
        })
        .collect::<Vec<_>>()
}

struct Matrix {
    value: Vec<Vec<Piece>>,
    a: usize,
    b: usize,
}

impl Matrix {
    pub fn new(seq_a: &LinearPeptide, seq_b: &LinearPeptide) -> Self {
        let a = seq_a.len();
        let b = seq_b.len();
        Self {
            value: vec![vec![Piece::default(); b + 1]; a + 1],
            a,
            b,
        }
    }

    #[allow(clippy::cast_possible_wrap)]
    pub fn global_start(&mut self, is_a: bool) {
        let max = if is_a { self.a } else { self.b };
        for index in 0..=max {
            self.value[if is_a { index } else { 0 }][if is_a { 0 } else { index }] = Piece::new(
                (index as isize) * GAP_EXTEND_PENALTY as isize,
                GAP_EXTEND_PENALTY,
                MatchType::Gap,
                u8::from(index != 0),
                0,
            );
        }
    }

    pub fn trace_path(
        &self,
        ty: Type,
        high: (isize, usize, usize),
    ) -> (isize, usize, usize, Vec<Piece>) {
        let mut path = Vec::new();
        let mut high = self.find_end(ty, high);
        let high_score = high.0;

        // Loop back to left side
        while ty.left_a() && ty.left_b() || !(high.1 == 0 && high.2 == 0) {
            let value = self.value[high.1][high.2].clone();
            if value.step_a == 0 && value.step_b == 0
                || !ty.left_a() && !ty.left_b() && value.score < 0
            {
                break;
            }
            high = (
                0,
                high.1 - value.step_a as usize,
                high.2 - value.step_b as usize,
            );
            path.push(value);
        }
        (high_score, high.1, high.2, path.into_iter().rev().collect())
    }

    fn find_end(&self, ty: Type, high: (isize, usize, usize)) -> (isize, usize, usize) {
        if ty.right_a() && ty.right_b() {
            (self.value[self.a][self.b].score, self.a, self.b)
        } else if ty.right_b() {
            let value = (0..=self.a)
                .map(|v| (v, self.value[v][self.b].score))
                .max_by(|a, b| a.1.cmp(&b.1))
                .unwrap_or_default();
            (value.1, value.0, self.b)
        } else if ty.right_a() {
            let value = (0..=self.b)
                .map(|v| (v, self.value[self.a][v].score))
                .max_by(|a, b| a.1.cmp(&b.1))
                .unwrap_or_default();
            (value.1, self.a, value.0)
        } else {
            high
        }
    }
}

impl std::ops::Index<(usize, usize)> for Matrix {
    type Output = Piece;
    fn index(&self, index: (usize, usize)) -> &Self::Output {
        assert!(index.0 <= self.a + 1);
        assert!(index.1 <= self.b + 1);
        &self.value[index.0][index.1]
    }
}
impl std::ops::IndexMut<(usize, usize)> for Matrix {
    fn index_mut(&mut self, index: (usize, usize)) -> &mut Self::Output {
        assert!(index.0 <= self.a + 1);
        assert!(index.1 <= self.b + 1);
        &mut self.value[index.0][index.1]
    }
}

#[cfg(test)]
mod tests {
    use super::score;
    use crate::aminoacids::AminoAcid;
    use crate::{MultiMolecularFormula, SequenceElement};

    #[test]
    fn pair() {
        let a = [SequenceElement::new(AminoAcid::N, None)];
        let b = [
            SequenceElement::new(AminoAcid::G, None),
            SequenceElement::new(AminoAcid::G, None),
        ];
        let pair = dbg!(score(
            (
                &a,
                &[a.iter()
                    .map(SequenceElement::formulas_all)
                    .sum::<MultiMolecularFormula>()[0]
                    .monoisotopic_mass()]
            ),
            (
                &b,
                &[b.iter()
                    .map(SequenceElement::formulas_all)
                    .sum::<MultiMolecularFormula>()[0]
                    .monoisotopic_mass()]
            ),
            0,
            crate::MassTolerance::Ppm(10.0)
        ));
        assert!(pair.is_some());
    }
}
