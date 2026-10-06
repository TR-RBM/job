const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encoded(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        for place in 0..4 {
            if place <= chunk.len() {
                text.push(ALPHABET[(group >> (18 - 6 * place) & 63) as usize] as char);
            } else {
                text.push('=');
            }
        }
    }
    text
}

fn value(symbol: u8) -> Option<u32> {
    ALPHABET
        .iter()
        .position(|known| *known == symbol)
        .map(|at| at as u32)
}

pub fn decoded(text: &str) -> Option<Vec<u8>> {
    let symbols = text.as_bytes();
    if !symbols.len().is_multiple_of(4) {
        return None;
    }
    let mut bytes = Vec::with_capacity(symbols.len() / 4 * 3);
    let groups = symbols.len() / 4;
    for (number, chunk) in symbols.chunks(4).enumerate() {
        let padding = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && number + 1 != groups) {
            return None;
        }
        let mut group = 0u32;
        for symbol in &chunk[..4 - padding] {
            group = group << 6 | value(*symbol)?;
        }
        group <<= 6 * padding as u32;
        bytes.push((group >> 16) as u8);
        if padding < 2 {
            bytes.push((group >> 8) as u8);
        }
        if padding < 1 {
            bytes.push(group as u8);
        }
    }
    Some(bytes)
}
