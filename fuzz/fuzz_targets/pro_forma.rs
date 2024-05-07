use afl::*;

fn main() {
    fuzz!(|data: &[u8]| {
        if let Ok(s) = std::str::from_utf8(data) {
            let _ = rustyms::CompoundPeptidoform::pro_forma(s, None);
        }
    });
}
