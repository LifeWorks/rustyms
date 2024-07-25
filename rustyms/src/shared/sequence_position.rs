/// A position on a sequence
#[derive(
    Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, serde::Serialize, serde::Deserialize,
)]
pub enum SequencePosition {
    /// N-terminal
    NTerm,
    /// An amino acid at the given index
    Index(usize),
    /// C-terminal
    CTerm,
}

impl Default for SequencePosition {
    fn default() -> Self {
        Self::Index(0)
    }
}

impl std::fmt::Display for SequencePosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NTerm => write!(f, "N-terminal"),
            Self::Index(index) => write!(f, "{index}"),
            Self::CTerm => write!(f, "C-terminal"),
        }
    }
}