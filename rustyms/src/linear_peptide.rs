#![warn(dead_code)]

use std::{
    fmt::Display,
    num::NonZeroU16,
    ops::{Index, RangeBounds},
    slice::SliceIndex,
};

use crate::{
    error::{Context, CustomError},
    fragment::{DiagnosticPosition, PeptidePosition},
    helper_functions::{end_of_enclosure, ResultExtensions},
    modification::{AmbiguousModification, GlobalModification, GnoComposition, ReturnModification},
    molecular_charge::MolecularCharge,
    placement_rule::PlacementRule,
    ComplexPeptide, DiagnosticIon, Element, MolecularFormula, Multi, MultiChemical, NeutralLoss,
    Protease, SequenceElement,
};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use uom::num_traits::Zero;

use crate::{
    aminoacids::AminoAcid, fragment::Fragment, fragment::FragmentType, modification::Modification,
    system::usize::Charge, Chemical, Model,
};

/// A peptide with all data as provided by pro forma. Preferably generated by using the [`crate::ComplexPeptide::pro_forma`] function.
#[derive(Default, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize, Hash)]
pub struct LinearPeptide {
    /// Global isotope modifications, saved as the element and the species that
    /// all occurrence of that element will consist of. Eg (N, 15) will make
    /// all occurring nitrogens be isotope 15.
    global: Vec<(Element, Option<NonZeroU16>)>,
    /// Labile modifications, which will not be found in the actual spectrum.
    pub labile: Vec<Modification>,
    /// N terminal modification
    pub n_term: Option<Modification>,
    /// C terminal modification
    pub c_term: Option<Modification>,
    /// The sequence of this peptide (includes local modifications)
    pub sequence: Vec<SequenceElement>,
    /// For each ambiguous modification list all possible positions it can be placed on.
    /// Indexed by the ambiguous modification id.
    pub ambiguous_modifications: Vec<Vec<usize>>,
    /// The adduct ions, if specified
    pub charge_carriers: Option<MolecularCharge>,
}

/// Builder style methods to create a [`LinearPeptide`]
impl LinearPeptide {
    /// Create a new [`LinearPeptide`], if you want an empty peptide look at [`LinearPeptide::default`].
    /// Potentially the `.collect()` or `.into()` methods can be useful as well.
    #[must_use]
    pub fn new(sequence: impl IntoIterator<Item = SequenceElement>) -> Self {
        sequence.into_iter().collect()
    }

    /// Add global isotope modifications, if any is invalid it returns None
    #[must_use]
    pub fn global(
        mut self,
        global: impl IntoIterator<Item = (Element, Option<NonZeroU16>)>,
    ) -> Option<Self> {
        for modification in global {
            if modification.0.is_valid(modification.1) {
                self.global.push(modification);
            } else {
                return None;
            }
        }
        Some(self)
    }

    /// Add labile modifications
    #[must_use]
    pub fn labile(mut self, labile: impl IntoIterator<Item = Modification>) -> Self {
        self.labile.extend(labile);
        self
    }

    /// Add the N terminal modification
    #[must_use]
    pub fn n_term(mut self, term: Option<Modification>) -> Self {
        self.n_term = term;
        self
    }

    /// Add the C terminal modification
    #[must_use]
    pub fn c_term(mut self, term: Option<Modification>) -> Self {
        self.c_term = term;
        self
    }

    /// Add the charge carriers
    #[must_use]
    pub fn charge_carriers(mut self, charge: Option<MolecularCharge>) -> Self {
        self.charge_carriers = charge;
        self
    }
}

impl LinearPeptide {
    /// Convenience wrapper to parse a linear peptide in pro forma notation, to handle all possible pro forma sequences look at [`ComplexPeptide::pro_forma`].
    /// # Errors
    /// It gives an error when the peptide is not correctly formatted. (Also see the `ComplexPeptide` main function for this.)
    /// It additionally gives an error if the peptide specified was chimeric (see [`ComplexPeptide::singular`]).
    pub fn pro_forma(value: &str) -> Result<Self, CustomError> {
        let complex = ComplexPeptide::pro_forma(value)?;
        complex.singular().ok_or_else(|| {
            CustomError::error(
                "Complex peptide found",
                "A linear peptide was expected but a chimeric peptide was found.",
                crate::error::Context::Show {
                    line: value.to_string(),
                },
            )
        })
    }

    /// Read sloppy pro forma like sequences. Defined by the use of square or round braces to indicate
    /// modifications and missing any particular method of defining the N or C terminal modifications.
    /// Additionally any underscores will be ignored both on the ends and inside the sequence.
    ///
    /// All modifications follow the same definitions as the strict pro forma syntax, if it cannot be
    /// parsed as a strict pro forma modification it falls back to [`Modification::sloppy_modification`].
    ///
    /// # Errors
    /// If it does not fit the above description.
    pub fn sloppy_pro_forma(
        line: &str,
        location: std::ops::Range<usize>,
    ) -> Result<Self, CustomError> {
        if line[location.clone()].trim().is_empty() {
            return Err(CustomError::error(
                "Peptide sequence is empty",
                "A peptide sequence cannot be empty",
                Context::line(0, line, location.start, 1),
            ));
        }
        let mut peptide = Self::default();
        let mut ambiguous_lookup = Vec::new();
        let chars: &[u8] = line[location.clone()].as_bytes();
        let mut index = 0;

        while index < chars.len() {
            match chars[index] {
                b'_' => index += 1, //ignore
                b'[' | b'(' => {
                    let (open, close) = if chars[index] == b'[' {
                        (b'[', b']')
                    } else {
                        (b'(', b')')
                    };
                    let end_index =
                        end_of_enclosure(chars, index + 1, open, close).ok_or_else(|| {
                            CustomError::error(
                                "Invalid modification",
                                "No valid closing delimiter",
                                Context::line(0, line, location.start + index, 1),
                            )
                        })?;
                    let modification = Modification::try_from(
                        line,
                        location.start + index + 1..location.start + end_index,
                        &mut ambiguous_lookup,
                    )
                    .map(|m| {
                        m.defined().ok_or_else(|| {
                            CustomError::error(
                                "Invalid modification",
                                "A modification in the sloppy peptide format cannot be ambiguous",
                                Context::line(
                                    0,
                                    line,
                                    location.start + index + 1,
                                    end_index - 1 - index,
                                ),
                            )
                        })
                    })
                    .flat_err()
                    .map_err(|err| {
                        Modification::sloppy_modification(
                            line,
                            location.start + index + 1..location.start + end_index,
                            peptide.sequence.last(),
                        )
                        .ok_or(err)
                    })
                    .flat_err()?;
                    index = end_index + 1;

                    match peptide.sequence.last_mut() {
                        Some(aa) => aa.modifications.push(modification),
                        None => peptide.n_term = Some(modification),
                    }
                }
                ch => {
                    peptide.sequence.push(SequenceElement::new(
                        ch.try_into().map_err(|()| {
                            CustomError::error(
                                "Invalid amino acid",
                                "This character is not a valid amino acid",
                                Context::line(0, line, location.start + index, 1),
                            )
                        })?,
                        None,
                    ));
                    index += 1;
                }
            }
        }
        peptide.enforce_modification_rules()?;
        Ok(peptide)
    }

    /// Get the number of amino acids making up this peptide
    pub fn len(&self) -> usize {
        self.sequence.len()
    }

    /// Check if there are any amino acids in this peptide
    pub fn is_empty(&self) -> bool {
        self.sequence.is_empty()
    }

    /// The mass of the N terminal modifications. The global isotope modifications are NOT applied.
    pub fn get_n_term(&self) -> MolecularFormula {
        self.n_term.as_ref().map_or_else(
            || molecular_formula!(H 1),
            |m| molecular_formula!(H 1) + m.formula(),
        )
    }

    /// The mass of the C terminal modifications. The global isotope modifications are NOT applied.
    pub fn get_c_term(&self) -> MolecularFormula {
        self.c_term.as_ref().map_or_else(
            || molecular_formula!(H 1 O 1),
            |m| molecular_formula!(H 1 O 1) + m.formula(),
        )
    }

    /// Get the global isotope modifications
    pub fn get_global(&self) -> &[(Element, Option<NonZeroU16>)] {
        &self.global
    }

    /// Get the reverse of this peptide
    #[must_use]
    pub fn reverse(&self) -> Self {
        Self {
            n_term: self.c_term.clone(),
            c_term: self.n_term.clone(),
            sequence: self.sequence.clone().into_iter().rev().collect(),
            ambiguous_modifications: self
                .ambiguous_modifications
                .clone()
                .into_iter()
                .map(|m| m.into_iter().map(|loc| self.len() - loc).collect())
                .collect(),
            ..self.clone()
        }
    }

    /// Assume that the underlying peptide does not use fancy parts of the Pro Forma spec. This is the common lower bound for support in all functions of rustyms.
    /// If you want to be even more strict on the kind of peptides you want to take take a look at [`Self::assume_very_simple`].
    /// # Panics
    /// When any of these functions are used:
    /// * Labile modifications
    /// * Global isotope modifications
    /// * Charge carriers, use of charged ions apart from protons
    /// * or when the sequence is empty.
    pub fn assume_simple(&self) {
        assert!(
            self.labile.is_empty(),
            "A simple linear peptide was assumed, but it has labile modifications"
        );
        assert!(
            self.global.is_empty(),
            "A simple linear peptide was assumed, but it has global isotope modifications"
        );
        assert!(
            self.charge_carriers.is_none(),
            "A simple linear peptide was assumed, but it has specified charged ions"
        );
        assert!(
            !self.sequence.is_empty(),
            "A simple linear peptide was assumed, but it has no sequence"
        );
    }

    /// Assume that the underlying peptide does not use fancy parts of the Pro Forma spec.
    /// # Panics
    /// When any of these functions are used:
    /// * Ambiguous modifications
    /// * Labile modifications
    /// * Global isotope modifications
    /// * Ambiguous amino acids (B/Z)
    /// * Ambiguous amino acid sequence `(?AA)`
    /// * Charge carriers, use of charged ions apart from protons
    /// * or when the sequence is empty.
    pub fn assume_very_simple(&self) {
        assert!(
            self.ambiguous_modifications.is_empty(),
            "A simple linear peptide was assumed, but it has ambiguous modifications"
        );
        assert!(
            self.labile.is_empty(),
            "A simple linear peptide was assumed, but it has labile modifications"
        );
        assert!(
            self.global.is_empty(),
            "A simple linear peptide was assumed, but it has global isotope modifications"
        );
        assert!(
            !self
                .sequence
                .iter()
                .any(|seq| seq.aminoacid == AminoAcid::B || seq.aminoacid == AminoAcid::Z),
            "A simple linear peptide was assumed, but it has ambiguous amino acids (B/Z)"
        );
        assert!(
            !self.sequence.iter().any(|seq| seq.ambiguous.is_some()),
            "A simple linear peptide was assumed, but it has ambiguous amino acids `(?AA)`"
        );
        assert!(
            self.charge_carriers.is_none(),
            "A simple linear peptide was assumed, but it has specified charged ions"
        );
        assert!(
            !self.sequence.is_empty(),
            "A simple linear peptide was assumed, but it has no sequence"
        );
    }

    /// # Errors
    /// If a modification rule is broken it returns an error.
    pub(crate) fn enforce_modification_rules(&self) -> Result<(), CustomError> {
        for (position, seq) in self.iter(..) {
            seq.enforce_modification_rules(&position)?;
        }
        Ok(())
    }

    /// Generate all possible patterns for the ambiguous positions (Mass, String:Label).
    /// It always contains at least one pattern (being (base mass, "")).
    /// The global isotope modifications are NOT applied.
    fn ambiguous_patterns(
        &self,
        range: impl RangeBounds<usize>,
        aa_range: impl RangeBounds<usize>,
        index: usize,
        base: MolecularFormula,
    ) -> Vec<(MolecularFormula, String)> {
        let result = self
            .ambiguous_modifications
            .iter()
            .enumerate()
            .fold(vec![Vec::new()], |acc, (id, possibilities)| {
                acc.into_iter()
                    .flat_map(|path| {
                        let mut path_clone = path.clone();
                        let options = possibilities.iter().filter(|pos| range.contains(pos)).map(
                            move |pos| {
                                let mut new = path.clone();
                                new.push((id, *pos));
                                new
                            },
                        );
                        options.chain(possibilities.iter().find(|pos| !range.contains(pos)).map(
                            move |pos| {
                                path_clone.push((id, *pos));
                                path_clone
                            },
                        ))
                    })
                    .collect()
            })
            .into_iter()
            .flat_map(|pattern| {
                let ambiguous_local = pattern
                    .iter()
                    .filter_map(|(id, pos)| (*pos == index).then_some(id))
                    .collect::<Vec<_>>();
                self.sequence[(
                    aa_range.start_bound().cloned(),
                    aa_range.end_bound().cloned(),
                )]
                    .iter()
                    .enumerate()
                    .fold(Multi::default(), |acc, (index, aa)| {
                        acc * aa.formulas(
                            &pattern
                                .clone()
                                .iter()
                                .copied()
                                .filter_map(|(id, pos)| (pos == index).then_some(id))
                                .collect_vec(),
                        )
                    })
                    .iter()
                    .map(|m| {
                        &base
                            + m
                            + self.sequence[index]
                                .possible_modifications
                                .iter()
                                .filter(|&am| ambiguous_local.contains(&&am.id))
                                .map(|am| am.modification.formula())
                                .sum::<MolecularFormula>()
                    })
                    .map(|m| {
                        (
                            m,
                            pattern.iter().fold(String::new(), |acc, (id, pos)| {
                                format!(
                                    "{acc}{}{}@{}",
                                    if acc.is_empty() { "" } else { "," },
                                    &self.sequence[index]
                                        .possible_modifications
                                        .iter()
                                        .find(|am| am.id == *id)
                                        .map_or(String::new(), |v| v
                                            .group
                                            .as_ref()
                                            .map_or(id.to_string(), |g| g.0.clone())),
                                    pos + 1
                                )
                            }),
                        )
                    })
                    .collect_vec()
            })
            .collect::<Vec<(MolecularFormula, String)>>();
        if result.is_empty() {
            vec![(base, String::new())]
        } else {
            result
        }
    }

    /// Gives all the formulas for the whole peptide with no C and N terminal modifications. With the global isotope modifications applied.
    #[allow(clippy::missing_panics_doc)] // global isotope mods are guaranteed to be correct
    pub fn bare_formulas(&self) -> Multi<MolecularFormula> {
        let mut formulas = Multi::default();
        let mut placed = vec![false; self.ambiguous_modifications.len()];
        for pos in &self.sequence {
            formulas *= pos.formulas_greedy(&mut placed);
        }

        formulas
            .iter()
            .map(|f| {
                f.with_global_isotope_modifications(&self.global)
                    .expect("Invalid global isotope modification in bare_formulas")
            })
            .collect()
    }

    /// Generate the theoretical fragments for this peptide, with the given maximal charge of the fragments, and the given model.
    /// With the global isotope modifications applied.
    ///
    /// # Panics
    /// If `max_charge` outside the range `1..=u64::MAX`.
    pub fn generate_theoretical_fragments(
        &self,
        max_charge: Charge,
        model: &Model,
    ) -> Vec<Fragment> {
        self.generate_theoretical_fragments_inner(max_charge, model, 0)
    }

    /// Generate the theoretical fragments for this peptide, with the given maximal charge of the fragments, and the given model.
    /// With the global isotope modifications applied.
    pub(crate) fn generate_theoretical_fragments_inner(
        &self,
        max_charge: Charge,
        model: &Model,
        peptide_index: usize,
    ) -> Vec<Fragment> {
        let default_charge = MolecularCharge::proton(max_charge.value as isize);
        let charge_carriers = self.charge_carriers.as_ref().unwrap_or(&default_charge);
        let single_charges = charge_carriers.all_single_charge_options();

        let mut output = Vec::with_capacity(20 * self.sequence.len() + 75); // Empirically derived required size of the buffer (Derived from Hecklib)
        for index in 0..self.sequence.len() {
            let position = PeptidePosition::n(index, self.len());
            let n_term = self.all_masses(
                ..=index,
                ..index,
                index,
                self.get_n_term(),
                model.modification_specific_neutral_losses,
            );
            let c_term = self.all_masses(
                index..,
                index + 1..,
                index,
                self.get_c_term(),
                model.modification_specific_neutral_losses,
            );
            let modifications_total = self.sequence[index]
                .modifications
                .iter()
                .map(Chemical::formula)
                .sum();

            output.append(&mut self.sequence[index].aminoacid.fragments(
                &n_term,
                &c_term,
                &modifications_total,
                charge_carriers,
                index,
                self.sequence.len(),
                &model.ions(position),
                peptide_index,
            ));

            if model.m {
                // m fragment: precursor amino acid side chain losses
                output.extend(self.formulas().iter().flat_map(|m| {
                    self.sequence[index]
                        .aminoacid
                        .formulas()
                        .iter()
                        .map(|aa| {
                            Fragment::new(
                                m.clone() - aa.clone() - modifications_total.clone()
                                    + molecular_formula!(C 2 H 2 N 1 O 1),
                                Charge::zero(),
                                peptide_index,
                                FragmentType::m(position, self.sequence[index].aminoacid),
                                String::new(),
                            )
                            .with_charge(charge_carriers)
                        })
                        .collect_vec()
                }));
            }
        }
        for fragment in &mut output {
            fragment.formula = fragment
                .formula
                .with_global_isotope_modifications(&self.global)
                .expect("Invalid global isotope modification");
        }

        // Generate precursor peak
        output.extend(self.formulas().iter().flat_map(|m| {
            Fragment::new(
                m.clone(),
                Charge::zero(),
                peptide_index,
                FragmentType::precursor,
                String::new(),
            )
            .with_charge(charge_carriers)
            .with_neutral_losses(&model.precursor)
        }));

        // Add glycan fragmentation to all peptide fragments
        // Assuming that only one glycan can ever fragment at the same time,
        // and that no peptide fragmentation occurs during glycan fragmentation
        for (sequence_index, position) in self.sequence.iter().enumerate() {
            for modification in &position.modifications {
                if let Modification::GlycanStructure(glycan) = modification {
                    output.extend(
                        glycan
                            .clone()
                            .determine_positions()
                            .generate_theoretical_fragments(
                                model,
                                peptide_index,
                                charge_carriers,
                                &self.formulas(),
                                (position.aminoacid, sequence_index),
                            ),
                    );
                } else if let Modification::Gno(GnoComposition::Structure(glycan), _) = modification
                {
                    output.extend(
                        glycan
                            .clone()
                            .determine_positions()
                            .generate_theoretical_fragments(
                                model,
                                peptide_index,
                                charge_carriers,
                                &self.formulas(),
                                (position.aminoacid, sequence_index),
                            ),
                    );
                }
            }
        }

        if model.modification_specific_diagnostic_ions {
            // Add all modification diagnostic ions
            output.extend(self.diagnostic_ions().into_iter().flat_map(|(dia, pos)| {
                Fragment {
                    formula: dia.0,
                    charge: Charge::default(),
                    ion: FragmentType::diagnostic(pos),
                    peptide_index,
                    neutral_loss: None,
                    label: String::new(),
                }
                .with_charges(&single_charges)
            }));
        }

        output
    }

    /// Generate all potential masses for the given stretch of amino acids.
    /// Applies ambiguous aminoacids and modifications, and neutral losses (if allowed in the model).
    fn all_masses(
        &self,
        range: impl RangeBounds<usize> + Clone,
        aa_range: impl RangeBounds<usize>,
        index: usize,
        base: MolecularFormula,
        apply_neutral_losses: bool,
    ) -> Vec<(MolecularFormula, String)> {
        let ambiguous_mods_masses = self.ambiguous_patterns(range.clone(), aa_range, index, base);
        if apply_neutral_losses {
            let neutral_losses = self.potential_neutral_losses(range);
            let mut all_masses =
                Vec::with_capacity(ambiguous_mods_masses.len() * (1 + neutral_losses.len()));
            all_masses.extend(ambiguous_mods_masses.iter().cloned());
            for loss in &neutral_losses {
                for option in &ambiguous_mods_masses {
                    all_masses.push((
                        &option.0 + &loss.0,
                        format!(
                            "{}{}{}({})",
                            option.1,
                            option.1.is_empty().then_some(",").unwrap_or_default(),
                            loss.0,
                            loss.1.sequence_index
                        ),
                    ));
                }
            }
            all_masses
        } else {
            ambiguous_mods_masses
        }
    }

    /// Find all neutral losses in the given stretch of peptide
    fn potential_neutral_losses(
        &self,
        range: impl RangeBounds<usize>,
    ) -> Vec<(NeutralLoss, PeptidePosition)> {
        let mut losses = Vec::new();
        for (pos, aa) in self.iter(range) {
            for modification in &aa.modifications {
                if let Modification::Predefined(_, rules, _, _, _) = modification {
                    for (rules, rule_losses, _) in rules {
                        if PlacementRule::any_possible(rules, aa, &pos) {
                            losses.extend(rule_losses.iter().map(|loss| (loss.clone(), pos)));
                        }
                    }
                }
            }
        }
        losses
    }

    /// Find all diagnostic ions for this full peptide
    fn diagnostic_ions(&self) -> Vec<(DiagnosticIon, DiagnosticPosition)> {
        let mut diagnostic = Vec::new();
        for (pos, aa) in self.iter(..) {
            for modification in &aa.modifications {
                if let Modification::Predefined(_, rules, _, _, _) = modification {
                    for (rules, _, rule_diagnostic) in rules {
                        if PlacementRule::any_possible(rules, aa, &pos) {
                            diagnostic.extend(rule_diagnostic.iter().map(|d| {
                                (
                                    d.clone(),
                                    crate::fragment::DiagnosticPosition::Peptide(pos, aa.aminoacid),
                                )
                            }));
                        }
                    }
                }
            }
        }
        for labile in &self.labile {
            if let Modification::Predefined(_, rules, _, _, _) = labile {
                for (_, _, rule_diagnostic) in rules {
                    diagnostic.extend(rule_diagnostic.iter().map(|d| {
                        (
                            d.clone(),
                            crate::fragment::DiagnosticPosition::Labile(labile.clone()),
                        )
                    }));
                }
            }
        }
        diagnostic
    }

    /// Iterate over a range in the peptide and keep track of the position
    fn iter(
        &self,
        range: impl RangeBounds<usize>,
    ) -> impl DoubleEndedIterator<Item = (PeptidePosition, &SequenceElement)> + '_ {
        let start = match range.start_bound() {
            std::ops::Bound::Unbounded => 0,
            std::ops::Bound::Included(i) => (*i).max(0),
            std::ops::Bound::Excluded(ex) => (ex + 1).max(0),
        };
        self.sequence[(range.start_bound().cloned(), range.end_bound().cloned())]
            .iter()
            .enumerate()
            .map(move |(index, seq)| (PeptidePosition::n(index + start, self.len()), seq))
    }

    /// Apply a global modification if this is a global isotope modification with invalid isotopes it returns false
    #[must_use]
    pub(crate) fn apply_global_modifications(
        &mut self,
        global_modifications: &[GlobalModification],
    ) -> bool {
        let length = self.len();
        for modification in global_modifications {
            match modification {
                GlobalModification::Fixed(pos, aa, modification) => {
                    for (_, seq) in self.sequence.iter_mut().enumerate().filter(|(index, seq)| {
                        pos.is_possible(&PeptidePosition::n(*index, length))
                            && seq.aminoacid.canonical_identical(*aa)
                            && modification.is_possible(seq, &PeptidePosition::n(*index, length))
                    }) {
                        seq.modifications.push(modification.clone());
                    }
                }
                GlobalModification::Free(modification) => {
                    for (_, seq) in self.sequence.iter_mut().enumerate().filter(|(index, seq)| {
                        modification.is_possible(seq, &PeptidePosition::n(*index, length))
                    }) {
                        seq.modifications.push(modification.clone());
                    }
                }
                GlobalModification::Isotope(el, isotope) if el.is_valid(*isotope) => {
                    self.global.push((*el, *isotope));
                }
                GlobalModification::Isotope(..) => return false,
            }
        }
        true
    }

    /// Place all global unknown positions at all possible locations as ambiguous modifications
    pub(crate) fn apply_unknown_position_modification(
        &mut self,
        unknown_position_modifications: &[Modification],
    ) {
        for modification in unknown_position_modifications {
            let id = self.ambiguous_modifications.len();
            let length = self.len();
            #[allow(clippy::unnecessary_filter_map)]
            // Side effects so the lint does not apply here
            self.ambiguous_modifications.push(
                (0..length)
                    .filter_map(|i| {
                        if modification
                            .is_possible(&self.sequence[i], &PeptidePosition::n(i, length))
                        {
                            self.sequence[i]
                                .possible_modifications
                                .push(AmbiguousModification {
                                    id,
                                    modification: modification.clone(),
                                    localisation_score: None,
                                    group: None,
                                });
                            Some(i)
                        } else {
                            None
                        }
                    })
                    .collect(),
            );
        }
    }
    /// Place all ranged unknown positions at all possible locations as ambiguous modifications
    /// # Panics
    /// It panics when information for an ambiguous modification is missing (name/mod).
    pub(crate) fn apply_ranged_unknown_position_modification(
        &mut self,
        ranged_unknown_position_modifications: &[(usize, usize, ReturnModification)],
        ambiguous_lookup: &[(Option<String>, Option<Modification>)],
    ) {
        for (start, end, ret_modification) in ranged_unknown_position_modifications {
            let (id, modification, score, group) = match ret_modification {
                ReturnModification::Defined(def) => {
                    self.ambiguous_modifications.push(Vec::new());
                    (
                        self.ambiguous_modifications.len() - 1,
                        def.clone(),
                        None,
                        None,
                    )
                }
                ReturnModification::Preferred(i, score) => {
                    if *i >= self.ambiguous_modifications.len() {
                        self.ambiguous_modifications.push(Vec::new());
                    }
                    (
                        *i,
                        ambiguous_lookup[*i].1.clone().unwrap(),
                        *score,
                        Some((ambiguous_lookup[*i].0.clone().unwrap(), true)), // TODO: now all possible location in the range are listed as preferred
                    )
                }
                ReturnModification::Referenced(i, score) => {
                    if *i >= self.ambiguous_modifications.len() {
                        self.ambiguous_modifications.push(Vec::new());
                    }
                    (
                        *i,
                        ambiguous_lookup[*i].1.clone().unwrap(),
                        *score,
                        Some((ambiguous_lookup[*i].0.clone().unwrap(), false)),
                    )
                }
            };
            let length = self.len();
            #[allow(clippy::unnecessary_filter_map)]
            // Side effects so the lint does not apply here
            let positions = (*start..=*end)
                .filter_map(|i| {
                    if modification.is_possible(&self.sequence[i], &PeptidePosition::n(i, length)) {
                        self.sequence[i]
                            .possible_modifications
                            .push(AmbiguousModification {
                                id,
                                modification: modification.clone(),
                                localisation_score: None,
                                group: group.clone(),
                            });
                        Some(i)
                    } else {
                        None
                    }
                })
                .collect_vec();
            if let Some(score) = score {
                let individual_score = score / positions.len() as f64;
                for pos in &positions {
                    self.sequence[*pos]
                        .possible_modifications
                        .last_mut()
                        .unwrap()
                        .localisation_score = Some(individual_score);
                }
            }
            self.ambiguous_modifications[id].extend(positions);
        }
    }

    /// Get a region of this peptide as a new peptide (with all terminal/global/ambiguous modifications).
    #[must_use]
    pub fn sub_peptide(&self, index: impl RangeBounds<usize>) -> Self {
        Self {
            n_term: if index.contains(&0) {
                self.n_term.clone()
            } else {
                None
            },
            c_term: if index.contains(&(self.len() - 1)) {
                self.c_term.clone()
            } else {
                None
            },
            sequence: self.sequence[(index.start_bound().cloned(), index.end_bound().cloned())]
                .to_vec(),
            ..self.clone()
        }
    }

    /// Digest this sequence with the given protease and the given maximal number of missed cleavages.
    pub fn digest(&self, protease: &Protease, max_missed_cleavages: usize) -> Vec<Self> {
        let mut sites = vec![0];
        sites.extend_from_slice(&protease.match_locations(&self.sequence));
        sites.push(self.len());

        let mut result = Vec::new();

        for (index, start) in sites.iter().enumerate() {
            for end in sites.iter().skip(index).take(max_missed_cleavages + 1) {
                result.push(self.sub_peptide((*start)..*end));
            }
        }
        result
    }
}

impl MultiChemical for LinearPeptide {
    /// Gives the formulas for the whole peptide. With the global isotope modifications applied. (Any B/Z will result in multiple possible formulas.)
    fn formulas(&self) -> Multi<MolecularFormula> {
        let mut formulas: Multi<MolecularFormula> =
            vec![self.get_n_term() + self.get_c_term()].into();
        let mut placed = vec![false; self.ambiguous_modifications.len()];
        for pos in &self.sequence {
            formulas *= pos.formulas_greedy(&mut placed);
        }

        formulas
            .iter()
            .map(|f| f.with_global_isotope_modifications(&self.global).expect("Global isotope modification invalid in determination of all formulas for a peptide"))
            .collect()
    }
}

impl Display for LinearPeptide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (element, isotope) in &self.global {
            write!(
                f,
                "<{}{}>",
                isotope.map(|i| i.to_string()).unwrap_or_default(),
                element
            )?;
        }
        for labile in &self.labile {
            write!(f, "{{{labile}}}")?;
        }
        if let Some(m) = &self.n_term {
            write!(f, "[{m}]-")?;
        }
        let mut placed = Vec::new();
        let mut last_ambiguous = None;
        for position in &self.sequence {
            placed.extend(position.display(f, &placed, last_ambiguous)?);
            last_ambiguous = position.ambiguous;
        }
        if last_ambiguous.is_some() {
            write!(f, ")")?;
        }
        if let Some(m) = &self.c_term {
            write!(f, "-[{m}]")?;
        }
        if let Some(c) = &self.charge_carriers {
            write!(f, "/{c}")?;
        }
        Ok(())
    }
}

impl<Collection, Item> From<Collection> for LinearPeptide
where
    Collection: IntoIterator<Item = Item>,
    Item: Into<SequenceElement>,
{
    fn from(value: Collection) -> Self {
        Self {
            global: Vec::new(),
            labile: Vec::new(),
            n_term: None,
            c_term: None,
            sequence: value.into_iter().map(std::convert::Into::into).collect(),
            ambiguous_modifications: Vec::new(),
            charge_carriers: None,
        }
    }
}

impl<Item> FromIterator<Item> for LinearPeptide
where
    Item: Into<SequenceElement>,
{
    fn from_iter<T: IntoIterator<Item = Item>>(iter: T) -> Self {
        Self::from(iter)
    }
}

impl<I: SliceIndex<[SequenceElement]>> Index<I> for LinearPeptide {
    type Output = I::Output;

    fn index(&self, index: I) -> &Self::Output {
        &self.sequence[index]
    }
}
