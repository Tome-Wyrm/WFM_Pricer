use aes::Aes128;
use cbc::Decryptor;
use cbc::cipher::{BlockDecryptMut, KeyIvInit};
use crate::AppResult;

type Aes128CbcDec = Decryptor<Aes128>;

pub const KEY: &[u8; 16] = b"LEO-ALEC\tEO-ALEC";
pub const IV: &[u8; 16] = &[
    49, 50, 70, 71, 66, 51, 54, 45, 76, 69, 51, 45, 113, 61, 57, 0,
];

/// Decrypts the given ciphertext using AES-128-CBC with PKCS7 padding.
///
/// # Errors
/// Returns an error if decryption fails due to invalid key, IV, or padding.
pub fn decrypt(ciphertext: &[u8]) -> AppResult<Vec<u8>> {
    let decryptor = Aes128CbcDec::new(KEY.into(), IV.into());
    let mut buf = ciphertext.to_vec();
    let decrypted = decryptor
        .decrypt_padded_mut::<cbc::cipher::block_padding::Pkcs7>(&mut buf)
        .map_err(|e| format!("Decryption error: {e:?}"))?;
    Ok(decrypted.to_vec())
}
