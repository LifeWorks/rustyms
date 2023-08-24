use std::collections::HashMap;

use uom::num_traits::Zero;

use crate::{
    fragment::Fragment,
    model::Model,
    peptide::Peptide,
    system::{f64::*, mass_over_charge::mz},
};

/// A raw spectrum (meaning not annotated yet)
#[derive(Clone, Debug)]
pub struct RawSpectrum {
    /// The title (as used in MGF)
    pub title: String,
    /// The number of scans
    pub num_scans: u64,
    /// The retention time
    pub rt: Time,
    /// The found precursor charge
    pub charge: Charge,
    /// The found precursor mass
    pub mass: Mass,
    /// The found precursor intensity
    pub intensity: Option<f64>,
    /// The peaks of which this spectrum consists
    pub spectrum: Vec<RawPeak>,
}

impl RawSpectrum {
    /// Filter the spectrum to retain all with an intensity above `filter_threshold` times the maximal intensity.
    ///
    /// # Panics
    /// It panics if any peaks has an intensity that is NaN.
    pub fn noise_filter(&mut self, filter_threshold: f64) {
        let max = self
            .spectrum
            .iter()
            .map(|p| p.intensity)
            .reduce(f64::max)
            .unwrap();
        self.spectrum
            .retain(|p| p.intensity >= max * filter_threshold);
        self.spectrum.shrink_to_fit();
    }

    /// Annotate this spectrum with the given peptide and given fragments see [`Peptide::generate_theoretical_fragments`].
    pub fn annotate(
        &self,
        peptide: Peptide,
        theoretical_fragments: &[Fragment],
        model: &Model,
    ) -> AnnotatedSpectrum {
        let mut annotated = AnnotatedSpectrum {
            title: self.title.clone(),
            num_scans: self.num_scans,
            rt: self.rt,
            charge: self.charge,
            mass: self.mass,
            peptide,
            spectrum: Vec::with_capacity(self.spectrum.len()),
        };

        let mut connections = Vec::with_capacity(self.spectrum.len());

        for (fragment_index, fragment) in theoretical_fragments.iter().enumerate() {
            connections.extend(self.spectrum.iter().enumerate().filter_map(|(i, p)| {
                p.ppm(fragment).and_then(|ppm| {
                    if ppm < model.ppm {
                        Some((
                            i,
                            fragment_index,
                            AnnotatedPeak::new(p, fragment.clone()),
                            ppm,
                        ))
                    } else {
                        None
                    }
                })
            }));
        }
        annotated.spectrum.extend(cluster_matches(
            connections,
            &self.spectrum,
            annotated.peptide.sequence.len(),
            model,
        ));

        annotated
    }
}

impl Default for RawSpectrum {
    fn default() -> Self {
        Self {
            title: String::new(),
            num_scans: 0,
            rt: Time::zero(),
            charge: Charge::new::<e>(1.0),
            mass: Mass::zero(),
            spectrum: Vec::new(),
            intensity: None,
        }
    }
}

type Connection = (usize, usize, AnnotatedPeak, MassOverCharge);

fn cluster_matches(
    matches: Vec<Connection>,
    spectrum: &[RawPeak],
    peptide_length: usize,
    model: &Model,
) -> Vec<AnnotatedPeak> {
    let mut found_peak_indices = HashMap::new();
    let mut found_fragment_indices = HashMap::new();
    for pair in &matches {
        *found_peak_indices.entry(pair.0).or_insert(0) += 1;
        *found_fragment_indices.entry(pair.1).or_insert(0) += 1;
    }
    let mut output = Vec::with_capacity(20 * peptide_length + 75); // Empirically derived required size of the buffer (Derived from Hecklib)
    let mut selected_peaks = Vec::new();
    let mut ambiguous = Vec::new();
    // First get all peaks that are unambiguously matched out of the selection to prevent a lot of computation
    for pair in matches {
        if found_peak_indices.get(&pair.0).map_or(false, |v| *v == 1)
            && found_fragment_indices
                .get(&pair.1)
                .map_or(false, |v| *v == 1)
        {
            output.push(pair.2);
            selected_peaks.push(pair.0);
        } else {
            ambiguous.push(pair);
        }
    }

    ambiguous.sort_unstable_by(|a, b| a.3.partial_cmp(&b.3).unwrap());

    // Now find all possible combinations of the ambiguous matches and get the non expandable set with the lowest total ppm error
    let mut sets = non_recursive_combinations(&ambiguous, model.ppm * peptide_length as f64);
    let max_number_connections =
        (found_peak_indices.len() - output.len()).min(found_fragment_indices.len() - output.len());
    for c in &mut sets {
        c.0 += (max_number_connections - c.1.len()) as f64 * MassOverCharge::new::<mz>(20.0);
    }
    let selected_set = sets.into_iter().fold(
        (MassOverCharge::new::<mz>(f64::INFINITY), Vec::new()),
        |acc, item| {
            if acc.0 > item.0 {
                item
            } else {
                acc
            }
        },
    );
    //dbg!(&selected_set);
    selected_peaks.extend(selected_set.1.iter().map(|c| c.0));
    output.extend(selected_set.1.into_iter().map(|c| c.2));
    selected_peaks.sort_unstable();
    output.extend(spectrum.iter().enumerate().filter_map(|(i, p)| {
        if selected_peaks.binary_search(&i).is_err() {
            Some(AnnotatedPeak::background(p))
        } else {
            None
        }
    }));
    output
}

/// Get all possible sets for the connection of a single extra time point
pub fn non_recursive_combinations(
    connections: &[Connection],
    ppm: MassOverCharge,
) -> Vec<(MassOverCharge, Vec<Connection>)> {
    let mut options: Vec<(MassOverCharge, Vec<usize>, usize)> = connections
        .iter()
        .enumerate()
        .map(|(i, c)| (c.3, vec![i], i))
        .collect();
    let mut finished = Vec::with_capacity(options.len());

    let mut next_options = Vec::with_capacity(options.len());
    let mut quit_threshold = ppm;
    loop {
        let mut changed = false;
        for option in &options {
            let mut found = false;
            let threshold_score =
                quit_threshold.mul_add(Ratio::new::<r>(option.1.len() as f64 + 1.0), -option.0);
            if threshold_score > MassOverCharge::zero() {
                for (index, connection) in connections
                    .iter()
                    .enumerate()
                    .skip(option.2)
                    .filter(|(_, connection)| {
                        connection.3 < threshold_score
                            && option.1.iter().all(|c| {
                                connections[*c].0 != connection.0
                                    && connections[*c].1 != connection.1
                            })
                    })
                    .take(1)
                {
                    let mut sel = option.1.clone();
                    sel.push(index);
                    next_options.push((option.0 + connection.3, sel, index + 1));
                    found = true;
                    changed = true;
                }
            }
            if !found {
                finished.push((option.0, option.1.clone()));
            }
        }
        options.clear();
        options.append(&mut next_options);
        if !changed {
            break;
        }
        quit_threshold = quit_threshold.min(
            options.iter().map(|o| o.0).sum::<MassOverCharge>()
                + finished.iter().map(|o| o.0).sum::<MassOverCharge>()
                    / Ratio::new::<r>((options.len() + finished.len()) as f64),
        );
    }

    finished
        .into_iter()
        .map(|(a, b)| (a, b.into_iter().map(|c| connections[c].clone()).collect()))
        .collect()
}

/// An annotated spectrum
#[derive(Clone, Debug)]
pub struct AnnotatedSpectrum {
    /// The title (as used in MGF)
    pub title: String,
    /// The number of scans
    pub num_scans: u64,
    /// The retention time
    pub rt: Time,
    /// The found precursor charge
    pub charge: Charge,
    /// The found precursor mass
    pub mass: Mass,
    /// The peptide with which this spectrum was annotated
    pub peptide: Peptide,
    /// The spectrum
    pub spectrum: Vec<AnnotatedPeak>,
}

/// A raw peak
#[derive(Clone, Debug)]
pub struct RawPeak {
    /// The mz value of this peak
    pub mz: MassOverCharge,
    /// The intensity of this peak
    pub intensity: f64,
    /// The charge of this peak
    pub charge: Charge, // #TODO: Is this item needed?
}

impl RawPeak {
    /// Determine the ppm error for the given fragment, optional because the mz of a [Fragment] is optional
    pub fn ppm(&self, fragment: &Fragment) -> Option<MassOverCharge> {
        Some(MassOverCharge::new::<mz>(self.mz.ppm(fragment.mz()?)))
    }
}

/// An annotated peak
#[derive(Clone, Debug)]
pub struct AnnotatedPeak {
    /// The experimental mz
    pub experimental_mz: MassOverCharge,
    /// The experimental intensity
    pub intensity: f64,
    /// The charge
    pub charge: Charge, // #TODO: Is this item needed?
    /// The annotation, if present
    pub annotation: Option<Fragment>,
}

impl AnnotatedPeak {
    /// Make a new annotated peak with the given annotation
    pub fn new(peak: &RawPeak, annotation: Fragment) -> Self {
        Self {
            experimental_mz: peak.mz,
            intensity: peak.intensity,
            charge: peak.charge,
            annotation: Some(annotation),
        }
    }

    /// Make a new annotated peak if no annotation is possible
    pub fn background(peak: &RawPeak) -> Self {
        Self {
            experimental_mz: peak.mz,
            intensity: peak.intensity,
            charge: peak.charge,
            annotation: None,
        }
    }
}
