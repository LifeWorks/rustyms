use crate::{
    error::{Context, CustomError},
    helper_functions::explain_number_error,
    Element, MolecularFormula, ELEMENT_PARSE_LIST,
};
use std::{num::NonZeroU16, ops::RangeBounds};

impl MolecularFormula {
    /// Parse Pro Forma formulas: `[13C2][12C-2]H2N`.
    /// # The specification (copied from Pro Forma v2)
    /// As no widely accepted specification exists for expressing elemental formulas, we have adapted a standard with the following rules (taken from <https://github.com/rfellers/chemForma>):
    /// ## Formula Rule 1
    /// A formula will be composed of pairs of atoms and their corresponding cardinality (two Carbon atoms: C2). Pairs SHOULD be separated by spaces but are not required to be.
    /// Atoms and cardinality SHOULD NOT be. Also, the Hill system for ordering (<https://en.wikipedia.org/wiki/Chemical_formula#Hill_system>) is preferred, but not required.
    /// ```text
    /// Example: C12H20O2 or C12 H20 O2
    /// ```
    /// ## Formula Rule 2
    /// Cardinalities must be positive or negative integer values. Zero is not supported. If a cardinality is not included with an atom, it is assumed to be +1.
    /// ```text
    /// Example: HN-1O2
    /// ```
    /// ## Formula Rule 3
    /// Isotopes will be handled by prefixing the atom with its isotopic number in square brackets. If no isotopes are specified, previous rules apply. If no isotope is specified, then it is
    /// assumed the natural isotopic distribution for a given element applies.
    /// ```text
    /// Example: [13C2][12C-2]H2N
    /// Example: [13C2]C-2H2N
    /// ```
    /// # Errors
    /// If the formula is not valid according to the above specification, with some help on what is going wrong.
    /// # Panics
    /// It can panic if the string contains not UTF8 symbols.
    #[allow(dead_code)]
    pub fn from_pro_forma(value: &str) -> Result<Self, CustomError> {
        Self::from_pro_forma_inner(value, .., false)
    }

    /// See [`Self::from_pro_forma`]. This is a variant to help in parsing a part of a larger line.
    /// # Errors
    /// If the formula is not valid according to the above specification, with some help on what is going wrong.
    /// # Panics
    /// It can panic if the string contains not UTF8 symbols.
    pub(crate) fn from_pro_forma_inner(
        value: &str,
        range: impl RangeBounds<usize>,
        allow_electrons: bool,
    ) -> Result<Self, CustomError> {
        let mut index = match range.start_bound() {
            std::ops::Bound::Unbounded => 0,
            std::ops::Bound::Included(s) => *s,
            std::ops::Bound::Excluded(s) => s + 1,
        };
        let end = match range.end_bound() {
            std::ops::Bound::Unbounded => value.len() - 1,
            std::ops::Bound::Included(s) => *s,
            std::ops::Bound::Excluded(s) => s - 1,
        };
        let mut element = None;
        let bytes = value.as_bytes();
        let mut result = Self::default();
        'main_parse_loop: while index <= end {
            match bytes[index] {
                b'[' => {
                    index += 1; // Skip the open square bracket
                    let len = bytes
                        .iter()
                        .skip(index)
                        .position(|c| *c == b']')
                        .ok_or_else(|| {
                            CustomError::error(
                                "Invalid Pro Forma molecular formula",
                                "No closing square bracket found",
                                Context::line(0, value, index, 1),
                            )
                        })?;
                    let isotope = bytes
                        .iter()
                        .skip(index)
                        .take_while(|c| c.is_ascii_digit())
                        .count();
                    let ele = bytes
                        .iter()
                        .skip(index + isotope)
                        .take_while(|c| c.is_ascii_alphabetic())
                        .count();

                    if allow_electrons
                        && (&bytes[index + isotope..index + isotope + ele] == b"e"
                            || &bytes[index + isotope..index + isotope + ele] == b"E")
                    {
                        element = Some(Element::Electron);
                    } else {
                        for possible in ELEMENT_PARSE_LIST {
                            if value[index + isotope..index + isotope + ele].to_ascii_lowercase()
                                == possible.0
                            {
                                element = Some(possible.1);
                                break;
                            }
                        }
                    };
                    if let Some(parsed_element) = element {
                        let num = value[index + isotope + ele..index + len]
                            .parse::<i32>()
                            .map_err(|err| {
                                CustomError::error(
                                    "Invalid Pro Forma molecular formula",
                                    format!("The element number {}", explain_number_error(&err)),
                                    Context::line(
                                        0,
                                        value,
                                        index + isotope + ele,
                                        len - isotope - ele,
                                    ),
                                )
                            })?;
                        let isotope = value[index..index + isotope]
                            .parse::<NonZeroU16>()
                            .map_err(|err| {
                                CustomError::error(
                                    "Invalid Pro Forma molecular formula",
                                    format!("The isotope number {}", explain_number_error(&err)),
                                    Context::line(0, value, index, isotope),
                                )
                            })?;

                        if !Self::add(&mut result, (parsed_element, Some(isotope), num)) {
                            return Err(CustomError::error(
                                "Invalid Pro Forma molecular formula",
                                format!("Invalid isotope ({isotope}) added for element ({parsed_element})"),
                                Context::line(0, value, index, len),
                            ),);
                        }
                        element = None;
                        index += len + 1;
                    } else {
                        return Err(CustomError::error(
                            "Invalid Pro Forma molecular formula",
                            "Invalid element",
                            Context::line(0, value, index + isotope, ele),
                        ));
                    }
                }
                b'-' | b'0'..=b'9' if element.is_some() => {
                    let (num, len) = std::str::from_utf8(
                        &bytes
                            .iter()
                            .skip(index)
                            .take(end - index + 1) // Bind the maximal length if this is used as part of the molecular charge parsing
                            .take_while(|c| c.is_ascii_digit() || **c == b'-')
                            .copied()
                            .collect::<Vec<_>>(),
                    )
                    .map_or_else(
                        |e| panic!("Non UTF8 in Pro Forma molecular formula, error: {e}"),
                        |v| {
                            (
                                v.parse::<i32>().map_err(|err| {
                                    CustomError::error(
                                        "Invalid Pro Forma molecular formula",
                                        format!(
                                            "The element number {}",
                                            explain_number_error(&err)
                                        ),
                                        Context::line(0, value, index, v.len()),
                                    )
                                }),
                                v.len(),
                            )
                        },
                    );
                    let num = num?;
                    if num != 0 && !Self::add(&mut result, (element.unwrap(), None, num)) {
                        return Err(CustomError::error(
                            "Invalid Pro Forma molecular formula",
                            format!(
                                "An element without a defined mass ({}) was used",
                                element.unwrap()
                            ),
                            Context::line(0, value, index - 1, 1),
                        ));
                    }
                    element = None;
                    index += len;
                }
                b' ' => index += 1,
                _ => {
                    if let Some(element) = element {
                        if !Self::add(&mut result, (element, None, 1)) {
                            return Err(CustomError::error(
                                "Invalid Pro Forma molecular formula",
                                format!("An element without a defined mass ({element}) was used"),
                                Context::line(0, value, index - 1, 1),
                            ));
                        }
                    }
                    for possible in ELEMENT_PARSE_LIST {
                        if value[index..(index + 2).min(value.len())]
                            .to_ascii_lowercase()
                            .starts_with(possible.0)
                        {
                            element = Some(possible.1);
                            index += possible.0.len();
                            continue 'main_parse_loop;
                        }
                    }
                    if allow_electrons && (bytes[index] == b'e' || bytes[index] == b'E') {
                        element = Some(Element::Electron);
                        index += 1;
                        continue 'main_parse_loop;
                    }
                    return Err(CustomError::error(
                        "Invalid Pro Forma molecular formula",
                        "Not a valid character in formula",
                        Context::line(0, value, index, 1),
                    ));
                }
            }
        }
        if let Some(element) = element {
            if !Self::add(&mut result, (element, None, 1)) {
                return Err(CustomError::error(
                    "Invalid Pro Forma molecular formula",
                    format!("An element without a defined mass ({element}) was used"),
                    Context::line(0, value, index - 1, 1),
                ));
            }
        }
        Ok(result)
    }
}