//! AES-128-ECB command encryption — doc/PLAN.md §10.3.
//!
//! `encrypt:1` means the inner message is
//! `base64(AES-128-ECB(json(message), session[:16]))`, with AES padding *disabled*
//! and the plaintext space-padded to a 16-byte multiple (CHANNEL_B_INBOUND.md).
//! We only ever encrypt: the device never does.

use aes::Aes128;
use aes::cipher::{Array, BlockCipherEncrypt, KeyInit};
use base64::Engine as _;
use serde_json::Value;

/// Encrypt an inner message for an `encrypt:1` envelope. `None` when the session key
/// is too short to hold an AES-128 key.
pub fn encrypt_message(message: &Value, session_key: &str) -> Option<String> {
    let key: [u8; 16] = session_key.as_bytes().get(..16)?.try_into().ok()?;
    let cipher = Aes128::new_from_slice(&key).ok()?;
    let mut plaintext = space_pad(&serde_json::to_vec(message).ok()?);

    for chunk in plaintext.as_chunks_mut::<16>().0 {
        let mut block = Array::from(*chunk);
        cipher.encrypt_block(&mut block);
        chunk.copy_from_slice(&block);
    }
    Some(base64::engine::general_purpose::STANDARD.encode(plaintext))
}

/// Space (`0x20`) padding to a 16-byte multiple; already-aligned input is untouched
/// (the firmware's decryptor has padding disabled and NUL-terminates the buffer).
fn space_pad(plaintext: &[u8]) -> Vec<u8> {
    let mut padded = plaintext.to_vec();
    let remainder = plaintext.len() % 16;
    if remainder != 0 {
        padded.extend(std::iter::repeat_n(b' ', 16 - remainder));
    }
    padded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    fn encrypt_block(key: &[u8; 16], block: [u8; 16]) -> Vec<u8> {
        let cipher = Aes128::new_from_slice(key).unwrap();
        let mut block = Array::from(block);
        cipher.encrypt_block(&mut block);
        block.to_vec()
    }

    #[test]
    fn fips_197_c1_vector() {
        let key: [u8; 16] = hex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let plaintext: [u8; 16] = hex("00112233445566778899aabbccddeeff").try_into().unwrap();
        assert_eq!(
            encrypt_block(&key, plaintext),
            hex("69c4e0d86a7b0430d8cdb78070b4c55a")
        );
    }

    #[test]
    fn aes_128_all_zero_vector() {
        assert_eq!(
            encrypt_block(&[0u8; 16], [0u8; 16]),
            hex("66e94bd4ef8a2c3b884cfa59ca342b2e")
        );
    }

    #[test]
    fn ecb_encrypts_each_block_independently() {
        let key = [7u8; 16];
        let block = [9u8; 16];
        assert_eq!(encrypt_block(&key, block), encrypt_block(&key, block));
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
    fn an_encrypted_message_is_well_formed() {
        let message = json!({
            "infoType": 21024,
            "data": {"cmd": "setledswitch", "value": 0},
            "dInfo": {"ts": "1", "userId": "probe"},
        });
        let key = "a".repeat(32);
        let encoded =
            encrypt_message(&message, &key).expect("32-character keys hold an AES-128 key");
        let ciphertext = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .unwrap();

        let plaintext_len = serde_json::to_vec(&message).unwrap().len();
        assert_eq!(
            ciphertext.len(),
            plaintext_len.div_ceil(16) * 16,
            "space padding lands on a block multiple"
        );
        assert_eq!(
            encrypt_message(&message, &key).unwrap(),
            encoded,
            "ECB encryption is deterministic"
        );
        assert!(encrypt_message(&message, "tooshort").is_none());
    }
}
