//! Stringhe UTF-16 per le API di Windows, senza allocazioni e senza `core::fmt`.

/// Stringa letterale UTF-16 calcolata a tempo di compilazione.
/// Restituisce `&'static [u16]` che INCLUDE lo zero finale (usabile come `PCWSTR`).
macro_rules! wide {
    ($s:literal) => {{
        const S: &str = $s;
        const N: usize = $crate::wide::utf16_len(S) + 1;
        const W: [u16; N] = $crate::wide::encode::<N>(S);
        &W as &'static [u16]
    }};
}

/// Numero di unità UTF-16 necessarie per `s` (senza terminatore).
pub const fn utf16_len(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    let mut n = 0;
    while i < b.len() {
        let (cp, len) = decode(b, i);
        n += if cp > 0xFFFF { 2 } else { 1 };
        i += len;
    }
    n
}

/// Codifica `s` in UTF-16 in un array di `N` unità (l'ultima resta 0).
pub const fn encode<const N: usize>(s: &str) -> [u16; N] {
    let b = s.as_bytes();
    let mut out = [0u16; N];
    let mut i = 0;
    let mut o = 0;
    while i < b.len() {
        let (cp, len) = decode(b, i);
        if cp > 0xFFFF {
            let c = cp - 0x1_0000;
            out[o] = 0xD800 + (c >> 10) as u16;
            out[o + 1] = 0xDC00 + (c & 0x3FF) as u16;
            o += 2;
        } else {
            out[o] = cp as u16;
            o += 1;
        }
        i += len;
    }
    out
}

/// Decodifica un carattere UTF-8 valido (le stringhe letterali Rust lo sono sempre).
const fn decode(b: &[u8], i: usize) -> (u32, usize) {
    let c = b[i] as u32;
    if c < 0x80 {
        (c, 1)
    } else if c < 0xE0 {
        (((c & 0x1F) << 6) | (b[i + 1] as u32 & 0x3F), 2)
    } else if c < 0xF0 {
        (((c & 0x0F) << 12) | ((b[i + 1] as u32 & 0x3F) << 6) | (b[i + 2] as u32 & 0x3F), 3)
    } else {
        let cp = ((c & 0x07) << 18)
            | ((b[i + 1] as u32 & 0x3F) << 12)
            | ((b[i + 2] as u32 & 0x3F) << 6)
            | (b[i + 3] as u32 & 0x3F);
        (cp, 4)
    }
}

/// Buffer UTF-16 a capacità fissa per costruire testi senza allocare (tooltip, messaggi).
/// Tiene sempre posto per lo zero finale: contiene al massimo `N - 1` unità.
#[derive(Clone, Copy)]
pub struct WBuf<const N: usize> {
    buf: [u16; N],
    len: usize,
}

impl<const N: usize> WBuf<N> {
    pub const fn new() -> WBuf<N> {
        WBuf { buf: [0; N], len: 0 }
    }

    pub fn clear(&mut self) {
        self.len = 0;
        if N > 0 {
            self.buf[0] = 0;
        }
    }

    /// Il contenuto senza lo zero finale.
    pub fn as_slice(&self) -> &[u16] {
        &self.buf[..self.len]
    }

    /// Il contenuto con lo zero finale, pronto per le API di Windows.
    pub fn as_wide(&self) -> &[u16] {
        &self.buf[..=self.len.min(N.saturating_sub(1))]
    }

    /// Aggiunge un'unità UTF-16; restituisce `false` (e non scrive) se il buffer è pieno.
    pub fn push(&mut self, c: u16) -> bool {
        if self.len + 1 >= N {
            return false;
        }
        self.buf[self.len] = c;
        self.len += 1;
        self.buf[self.len] = 0;
        true
    }

    /// Aggiunge una stringa UTF-16 (l'eventuale zero finale viene ignorato).
    pub fn push_wide(&mut self, s: &[u16]) {
        for &c in s.iter().take_while(|&&c| c != 0) {
            if !self.push(c) {
                return;
            }
        }
    }

    /// Aggiunge una stringa UTF-8 convertendola in UTF-16 al volo.
    pub fn push_str(&mut self, s: &str) {
        for c in s.chars() {
            let mut u = [0u16; 2];
            for &mut unit in c.encode_utf16(&mut u) {
                if !self.push(unit) {
                    return;
                }
            }
        }
    }

    /// Aggiunge un numero intero in base 10.
    pub fn push_u32(&mut self, mut v: u32) {
        let mut digits = [0u16; 10];
        let mut n = 0;
        loop {
            digits[n] = b'0' as u16 + (v % 10) as u16;
            n += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        while n > 0 {
            n -= 1;
            if !self.push(digits[n]) {
                return;
            }
        }
    }
}

impl<const N: usize> Default for WBuf<N> {
    fn default() -> WBuf<N> {
        WBuf::new()
    }
}

impl<const N: usize> PartialEq for WBuf<N> {
    fn eq(&self, other: &WBuf<N>) -> bool {
        self.as_slice() == other.as_slice()
    }
}

/// Copia `src` (senza lo zero finale) in `dst`, troncando e terminando sempre con zero.
pub fn copy_to_fixed(dst: &mut [u16], src: &[u16]) {
    if dst.is_empty() {
        return;
    }
    let n = src.iter().take_while(|&&c| c != 0).count().min(dst.len() - 1);
    dst[..n].copy_from_slice(&src[..n]);
    dst[n] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(w: &[u16]) -> String {
        String::from_utf16_lossy(w)
    }

    #[test]
    fn wide_literal_has_terminator() {
        let w = wide!("CPU");
        assert_eq!(w, &[b'C' as u16, b'P' as u16, b'U' as u16, 0]);
    }

    #[test]
    fn wide_literal_handles_accents_and_symbols() {
        let w = wide!("più ° — ↑");
        assert_eq!(s(&w[..w.len() - 1]), "più ° — ↑");
        assert_eq!(*w.last().unwrap(), 0);
    }

    #[test]
    fn wide_literal_handles_surrogates() {
        let w = wide!("a😀");
        assert_eq!(w.len(), 4);
        assert_eq!(s(&w[..3]), "a😀");
    }

    #[test]
    fn wbuf_builds_text_and_numbers() {
        let mut b = WBuf::<32>::new();
        b.push_wide(wide!("CPU "));
        b.push_u32(0);
        b.push(b' ' as u16);
        b.push_u32(4_294_967_295);
        assert_eq!(s(b.as_slice()), "CPU 0 4294967295");
    }

    #[test]
    fn wbuf_truncates_and_keeps_terminator() {
        let mut b = WBuf::<4>::new();
        b.push_wide(wide!("abcdef"));
        assert_eq!(s(b.as_slice()), "abc");
        assert_eq!(b.as_wide(), &[b'a' as u16, b'b' as u16, b'c' as u16, 0]);
        assert!(!b.push(b'x' as u16));
    }

    #[test]
    fn wbuf_equality_ignores_stale_bytes() {
        let mut a = WBuf::<8>::new();
        a.push_wide(wide!("abc"));
        a.clear();
        a.push_wide(wide!("x"));
        let mut b = WBuf::<8>::new();
        b.push_wide(wide!("x"));
        assert!(a == b);
    }

    #[test]
    fn copy_to_fixed_truncates() {
        let mut dst = [7u16; 3];
        copy_to_fixed(&mut dst, wide!("abcd"));
        assert_eq!(dst, [b'a' as u16, b'b' as u16, 0]);
    }
}
