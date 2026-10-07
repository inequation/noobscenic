//! AES-128-ECB command encryption — doc/PLAN.md §10.3.
//!
//! `encrypt:1` means `data` is `base64(AES-128-ECB(json(data), session[:16]))`, with
//! AES padding *disabled* and the plaintext space-padded to a 16-byte multiple
//! (PROTOCOL.md §B). We only ever encrypt: the device never does.
//!
//! The `aes` crate the plan names is not available offline, so this is a small
//! encrypt-only implementation, verified against the FIPS-197 test vectors below.
//! Swap it for the crate once the dependency can be fetched.

use std::sync::OnceLock;

use base64::Engine as _;
use serde_json::{Value, json};

/// The `encrypt:1` form of a command, or `None` when the session key is too short
/// to hold an AES-128 key.
pub fn encrypt_command(info_type: i64, data: &Value, session_key: &str) -> Option<Value> {
    let key: [u8; 16] = session_key.as_bytes().get(..16)?.try_into().ok()?;
    let plaintext = serde_json::to_vec(data).ok()?;
    let ciphertext = Aes128::new(&key).encrypt_ecb(&space_pad(&plaintext));
    let encoded = base64::engine::general_purpose::STANDARD.encode(ciphertext);
    Some(json!({"infoType": info_type, "encrypt": 1, "data": encoded}))
}

/// Space (`0x20`) padding to a 16-byte multiple; already-aligned input is untouched
/// (padding is disabled, per the firmware's decryptor).
fn space_pad(plaintext: &[u8]) -> Vec<u8> {
    let mut padded = plaintext.to_vec();
    let remainder = plaintext.len() % 16;
    if remainder != 0 {
        padded.extend(std::iter::repeat_n(b' ', 16 - remainder));
    }
    padded
}

struct Aes128 {
    round_keys: [u8; 176],
}

impl Aes128 {
    fn new(key: &[u8; 16]) -> Aes128 {
        let sbox = sbox();
        let mut w = [0u8; 176];
        w[..16].copy_from_slice(key);

        let mut rcon = 1u8;
        let mut i = 4;
        while i < 44 {
            let mut t = [w[i * 4 - 4], w[i * 4 - 3], w[i * 4 - 2], w[i * 4 - 1]];
            if i % 4 == 0 {
                t = [t[1], t[2], t[3], t[0]]; // RotWord
                for byte in &mut t {
                    *byte = sbox[*byte as usize]; // SubWord
                }
                t[0] ^= rcon;
                rcon = xtime(rcon);
            }
            for j in 0..4 {
                w[i * 4 + j] = w[(i - 4) * 4 + j] ^ t[j];
            }
            i += 1;
        }
        Aes128 { round_keys: w }
    }

    /// ECB over whole 16-byte blocks; the caller has padded the input already.
    fn encrypt_ecb(&self, data: &[u8]) -> Vec<u8> {
        debug_assert_eq!(data.len() % 16, 0, "space_pad aligns the plaintext");
        let mut out = Vec::with_capacity(data.len());
        let mut block = [0u8; 16];
        for chunk in data.chunks(16) {
            block.copy_from_slice(chunk);
            self.encrypt_block(&mut block);
            out.extend_from_slice(&block);
        }
        out
    }

    fn encrypt_block(&self, block: &mut [u8; 16]) {
        self.add_round_key(block, 0);
        for round in 1..10 {
            sub_bytes(block);
            shift_rows(block);
            mix_columns(block);
            self.add_round_key(block, round);
        }
        sub_bytes(block);
        shift_rows(block);
        self.add_round_key(block, 10);
    }

    fn add_round_key(&self, block: &mut [u8; 16], round: usize) {
        let key = &self.round_keys[round * 16..round * 16 + 16];
        for (byte, k) in block.iter_mut().zip(key) {
            *byte ^= *k;
        }
    }
}

fn sub_bytes(state: &mut [u8; 16]) {
    let sbox = sbox();
    for byte in state {
        *byte = sbox[*byte as usize];
    }
}

/// Row `r` rotates left by `r`; the state is column-major (`row + 4*col`).
fn shift_rows(state: &mut [u8; 16]) {
    let t = state[1];
    state[1] = state[5];
    state[5] = state[9];
    state[9] = state[13];
    state[13] = t;

    state.swap(2, 10);
    state.swap(6, 14);

    let t = state[15];
    state[15] = state[11];
    state[11] = state[7];
    state[7] = state[3];
    state[3] = t;
}

fn mix_columns(state: &mut [u8; 16]) {
    for column in 0..4 {
        let i = column * 4;
        let (a0, a1, a2, a3) = (state[i], state[i + 1], state[i + 2], state[i + 3]);
        state[i] = xtime(a0) ^ (xtime(a1) ^ a1) ^ a2 ^ a3;
        state[i + 1] = a0 ^ xtime(a1) ^ (xtime(a2) ^ a2) ^ a3;
        state[i + 2] = a0 ^ a1 ^ xtime(a2) ^ (xtime(a3) ^ a3);
        state[i + 3] = (xtime(a0) ^ a0) ^ a1 ^ a2 ^ xtime(a3);
    }
}

fn xtime(byte: u8) -> u8 {
    (byte << 1) ^ if byte & 0x80 != 0 { 0x1b } else { 0 }
}

fn sbox() -> &'static [u8; 256] {
    static SBOX: OnceLock<[u8; 256]> = OnceLock::new();
    SBOX.get_or_init(|| {
        let mut table = [0u8; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let inverse = if i == 0 { 0 } else { gf_pow(i as u8, 254) };
            *slot = inverse
                ^ inverse.rotate_left(1)
                ^ inverse.rotate_left(2)
                ^ inverse.rotate_left(3)
                ^ inverse.rotate_left(4)
                ^ 0x63;
        }
        table
    })
}

/// Multiplication in GF(2^8) modulo the AES polynomial 0x11b.
fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 {
            product ^= a;
        }
        b >>= 1;
        a = xtime(a);
    }
    product
}

fn gf_pow(base: u8, mut exponent: u32) -> u8 {
    let mut result = 1u8;
    let mut factor = base;
    while exponent > 0 {
        if exponent & 1 == 1 {
            result = gmul(result, factor);
        }
        factor = gmul(factor, factor);
        exponent >>= 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn fips_197_c1_vector() {
        let key: [u8; 16] = hex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let plaintext: [u8; 16] = hex("00112233445566778899aabbccddeeff").try_into().unwrap();
        let ciphertext = Aes128::new(&key).encrypt_ecb(&plaintext);
        assert_eq!(ciphertext, hex("69c4e0d86a7b0430d8cdb78070b4c55a"));
    }

    #[test]
    fn aes_128_all_zero_vector() {
        let ciphertext = Aes128::new(&[0u8; 16]).encrypt_ecb(&[0u8; 16]);
        assert_eq!(ciphertext, hex("66e94bd4ef8a2c3b884cfa59ca342b2e"));
    }

    #[test]
    fn ecb_encrypts_each_block_independently() {
        let key = [7u8; 16];
        let block = [9u8; 16];
        let mut two_blocks = block.to_vec();
        two_blocks.extend_from_slice(&block);
        let ciphertext = Aes128::new(&key).encrypt_ecb(&two_blocks);
        assert_eq!(ciphertext.len(), 32);
        assert_eq!(ciphertext[..16], ciphertext[16..]);
    }

    #[test]
    fn padding_is_spaces_and_never_adds_a_whole_block() {
        assert_eq!(space_pad(b"abc").len(), 16);
        assert!(space_pad(b"abc").ends_with(&[b' '; 13]));
        assert_eq!(
            space_pad(&[1u8; 16]),
            vec![1u8; 16],
            "aligned input is untouched"
        );
        assert_eq!(space_pad(b"").len(), 0);
    }

    #[test]
    fn an_encrypted_command_is_well_formed() {
        let frame = encrypt_command(
            21024,
            &json!({"cmd": "setledswitch", "value": 0}),
            &"a".repeat(32),
        )
        .expect("32-character keys hold an AES-128 key");
        assert_eq!(frame["infoType"], 21024);
        assert_eq!(frame["encrypt"], 1);
        let ciphertext = base64::engine::general_purpose::STANDARD
            .decode(frame["data"].as_str().unwrap())
            .unwrap();
        // `{"cmd":"setledswitch","value":0}` is 31 bytes, so one space pads it to 32.
        assert_eq!(
            ciphertext.len(),
            32,
            "space padding lands on a block multiple"
        );

        assert!(encrypt_command(21024, &json!({}), "tooshort").is_none());
    }
}
