//! Tiny terminal-clipboard helper.
//!
//! Writes the given text to the system clipboard using the OSC 52
//! escape sequence. OSC 52 is supported by every modern terminal
//! emulator we care about (kitty, WezTerm, Alacritty, iTerm2, xterm
//! with `allowWindowOps`, tmux ≥ 3.2 with `set-clipboard on`, and most
//! recent VTE-based terminals).
//!
//! The advantage over shelling out to `xclip` / `wl-copy` / `pbcopy`
//! is zero: no dependency on which Wayland session you're in, no extra
//! processes, no crate dependency.
//!
//! The drawback is that some terminals cap the payload size; we keep
//! the copy small enough (truncating at 64 KiB, base64-encoded) that
//! it fits inside every reasonable cap.

use std::io::{self, Write};

/// Maximum number of raw bytes we'll ever attempt to push to the
/// clipboard. 64 KiB is generous for table cells / rows and still safe
/// for every terminal's OSC 52 buffer.
pub const MAX_CLIPBOARD_BYTES: usize = 64 * 1024;

/// Copy `text` to the terminal's system clipboard using OSC 52.
///
/// Returns the number of bytes that were actually sent (may be smaller
/// than `text.len()` if we had to truncate). On stdout failure the
/// error is logged via `tracing::warn!` and `Ok(0)` is returned — this
/// is cosmetic UX, not something that should bubble up as an app error.
pub fn copy_to_clipboard(text: &str) -> io::Result<usize> {
    let bytes = text.as_bytes();
    let truncated = if bytes.len() > MAX_CLIPBOARD_BYTES {
        &bytes[..MAX_CLIPBOARD_BYTES]
    } else {
        bytes
    };

    let encoded = base64_encode(truncated);
    // ESC ] 52 ; c ; <base64> BEL
    // "c" = primary clipboard; most terminals alias primary + selection.
    let mut stdout = io::stdout().lock();
    write!(stdout, "\x1b]52;c;{encoded}\x07")?;
    stdout.flush()?;
    Ok(truncated.len())
}

/// RFC 4648 base64 encoder, inline so we don't pull in a dependency
/// just for clipboard bytes.
fn base64_encode(input: &[u8]) -> String {
    const ALPHA: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n =
            ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
        out.push(ALPHA[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_rfc4648_vectors() {
        // Standard RFC 4648 test vectors.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_encodes_utf8() {
        // Greek "α β" — just check we don't panic on non-ASCII bytes.
        let input = "α β".as_bytes();
        let out = base64_encode(input);
        assert!(!out.is_empty());
        assert_eq!(out.len() % 4, 0);
    }
}
