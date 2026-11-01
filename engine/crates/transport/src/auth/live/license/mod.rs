//! The device credential's cryptographic core.
//!
//! It covers the key-derivation function the token broker uses to sign and decrypt its requests,
//! the AES helpers, and the recovery of the device RSA key and the HMAC proof secret from the
//! `SPLicenseBlock` and proof token the account service returns.
//!
//! The account service hands these secrets to the device during registration for the device to use
//! on its own follow-up requests; this module reconstructs them exactly as the layout requires.

use aes::cipher::block_padding::{NoPadding, Pkcs7};
use aes::cipher::{BlockDecryptMut, KeyIvInit};
use hmac::{Hmac, KeyInit, Mac};
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, RsaPrivateKey};
use sha2::Sha256;

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;
type HmacSha256 = Hmac<Sha256>;

/// The KDF label: the ASCII string `WS-SecureConversation` written twice, with no separator.
pub const KDF_LABEL: &[u8] = b"WS-SecureConversationWS-SecureConversation";

#[derive(Debug, thiserror::Error)]
pub enum LicenseError {
    #[error("the encrypted payload is shorter than one AES block + IV")]
    ShortPayload,
    #[error("AES decryption failed (bad padding or length)")]
    Decrypt,
    #[error("SPLicenseBlock: {0}")]
    License(String),
    #[error("BCrypt RSA blob: {0}")]
    RsaBlob(String),
    #[error("the recovered RSA key is invalid: {0}")]
    RsaKey(String),
    #[error("the recovered RSA key is invalid: {0}")]
    RsaKeyInvalid(#[source] rsa::Error),
}

/// NIST SP800-108 counter-mode KDF with HMAC-SHA256 as the PRF, as the token broker's
/// `SP800108_CTR_HMAC_SHA256` derivation uses it.
///
/// The PRF input for each 32-byte output block is
/// `[counter:4 BE] || label || 0x00 || context || [L:4 BE]`, where `counter` starts at 1 and `L` is
/// the total output length in **bits**. `context` is the per-request nonce.
pub fn sp800_108(key: &[u8], label: &[u8], context: &[u8], out_len: usize) -> Vec<u8> {
    let l_bits = (out_len as u32) * 8;
    let mut out = Vec::with_capacity(out_len);
    let mut counter: u32 = 1;

    while out.len() < out_len {
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes a key of any length");
        mac.update(&counter.to_be_bytes());
        mac.update(label);
        mac.update(&[0u8]);
        mac.update(context);
        mac.update(&l_bits.to_be_bytes());
        out.extend_from_slice(&mac.finalize().into_bytes());
        counter += 1;
    }

    out.truncate(out_len);
    out
}

/// Decrypts a token-broker response body: AES-256-CBC, PKCS7, with the IV as the first 16 bytes of
/// the cipher value and the ciphertext the remainder.
pub fn decrypt_response(cipher_value: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, LicenseError> {
    let (iv, data) = cipher_value.split_at_checked(16).ok_or(LicenseError::ShortPayload)?;
    let mut buf = data.to_vec();
    let plain =
        Aes128or256::aes256(key, iv).decrypt_padded_mut::<Pkcs7>(&mut buf).map_err(|_| LicenseError::Decrypt)?;
    Ok(plain.to_vec())
}

/// Thin constructors so the two block sizes read the same way.
struct Aes128or256;
impl Aes128or256 {
    fn aes256(key: &[u8; 32], iv: &[u8]) -> Aes256CbcDec {
        Aes256CbcDec::new(key.into(), iv.into())
    }
}

/// Decrypts a CLEP `key_data` region: AES-128-CBC with an all-zero IV and no padding. The input
/// length must be a multiple of the AES block size (the CLEP regions always are).
fn decrypt_clep(key_data: &[u8], key: &[u8; 16]) -> Result<Vec<u8>, LicenseError> {
    if key_data.len() % 16 != 0 {
        return Err(LicenseError::Decrypt);
    }

    let mut buf = key_data.to_vec();
    let plain = Aes128CbcDec::new(key.into(), &[0u8; 16].into())
        .decrypt_padded_mut::<NoPadding>(&mut buf)
        .map_err(|_| LicenseError::Decrypt)?;
    Ok(plain.to_vec())
}

/// A CLEP block's AES-128 key, read out of four fixed words of the expanded key schedule `XORed` with
/// four fixed constants (the block does not carry the key directly). The 58-word schedule is laid
/// out little-endian in the block.
fn clep_aes_key(schedule: &[u32; 58]) -> [u8; 16] {
    let k0 = schedule[46] ^ schedule[56] ^ 0xE20D_F371 ^ 0xCCB2_2FE6;
    let k1 = schedule[36] ^ schedule[47] ^ 0xDF08_0E39;
    let k2 = schedule[40] ^ schedule[51] ^ 0x6D09_B2F5 ^ 0x2AE1_7AB9;
    let k3 = schedule[30] ^ schedule[41] ^ 0x3728_8CEC;

    let mut key = [0u8; 16];
    key[0..4].copy_from_slice(&k0.to_le_bytes());
    key[4..8].copy_from_slice(&k1.to_le_bytes());
    key[8..12].copy_from_slice(&k2.to_le_bytes());
    key[12..16].copy_from_slice(&k3.to_le_bytes());
    key
}

/// Reads the 58-word little-endian key schedule at `offset` in a 4096-byte CLEP block.
fn read_schedule(block: &[u8; 4096], offset: usize) -> [u32; 58] {
    let mut schedule = [0u32; 58];
    for (i, word) in schedule.iter_mut().enumerate() {
        let at = offset + i * 4;
        *word = u32::from_le_bytes([block[at], block[at + 1], block[at + 2], block[at + 3]]);
    }
    schedule
}

/// A 4096-byte CLEP block: a `version` word (`4`), a `key_data` ciphertext of `data_len` bytes at
/// offset 4, and the 58-word key schedule immediately after it. Decrypts `key_data`.
fn decrypt_clep_block(block: &[u8; 4096], data_len: usize) -> Result<Vec<u8>, LicenseError> {
    let version = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
    if version != 4 {
        return Err(LicenseError::License(format!("unsupported CLEP version {version}")));
    }

    let schedule = read_schedule(block, 4 + data_len);
    let key = clep_aes_key(&schedule);
    decrypt_clep(&block[4..4 + data_len], &key)
}

/// `ClepSignState` carries a 544-byte `key_data`; its plaintext is the device RSA private key as a
/// `BCrypt` blob.
const SIGN_STATE_DATA_LEN: usize = 544;
/// `ClepHmacState` carries a 48-byte `key_data`; its plaintext is a `BCrypt` symmetric key blob whose
/// 32 key bytes (after a 12-byte header) are the HMAC proof secret.
const HMAC_STATE_DATA_LEN: usize = 48;

/// Recovers the device RSA private key from a `ClepSignState` block (TLV id `0x12d` of the
/// `SPLicenseBlock`).
pub fn device_rsa_key(sign_state: &[u8; 4096]) -> Result<RsaPrivateKey, LicenseError> {
    let plain = decrypt_clep_block(sign_state, SIGN_STATE_DATA_LEN)?;
    parse_bcrypt_rsa_private(&plain)
}

/// Recovers the 32-byte HMAC proof secret from a `ClepHmacState` block (the response's proof token).
pub fn hmac_proof_secret(hmac_state: &[u8; 4096]) -> Result<[u8; 32], LicenseError> {
    let plain = decrypt_clep_block(hmac_state, HMAC_STATE_DATA_LEN)?;
    // BCrypt key blob: a 12-byte header (magic, version, key length) then the key bytes.
    plain
        .get(12..44)
        .ok_or_else(|| LicenseError::RsaBlob("HMAC key blob shorter than 44 bytes".into()))?
        .try_into()
        .map_err(|_| LicenseError::RsaBlob("HMAC key slice is not 32 bytes".into()))
}

/// The little-endian `u32` at `at`; the caller checked that four bytes are there.
fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("a four-byte slice"))
}

/// Walks the `SPLicenseBlock` TLV stream and returns its `ClepSignState` block (id `0x12d`). The
/// block is a 4-byte header + 4-byte offset, then records of `id: u32 LE`, `size: u32 LE`, `size`
/// bytes.
pub fn find_sign_state(sp_license_block: &[u8]) -> Result<[u8; 4096], LicenseError> {
    const CLEP_SIGN_STATE: u32 = 0x12d;
    let mut pos = 8usize; // skip the 4-byte header + 4-byte offset

    while pos + 8 <= sp_license_block.len() {
        let id = le_u32(sp_license_block, pos);
        let size = le_u32(sp_license_block, pos + 4) as usize;
        let body = pos + 8;
        let end = body.checked_add(size).ok_or_else(|| LicenseError::License("TLV size overflow".into()))?;

        if end > sp_license_block.len() {
            return Err(LicenseError::License("TLV record runs past the block".into()));
        }

        if id == CLEP_SIGN_STATE {
            return sp_license_block[body..end]
                .try_into()
                .map_err(|_| LicenseError::License(format!("ClepSignState is {size} bytes, not 4096")));
        }

        pos = end;
    }

    Err(LicenseError::License("no ClepSignState (0x12d) block".into()))
}

/// Parses a Microsoft `BCrypt` RSA private-key blob into an [`RsaPrivateKey`].
///
/// The header is six little-endian `u32`s — magic, bit length, public-exponent length, modulus
/// length, prime1 length, prime2 length — then the fields big-endian in order: `e`, `n`, `p`, `q`,
/// and for a full-private blob (`RSA3`) `d` of modulus length. A plain-private blob (`RSA2`) omits
/// `d`, so it is recomputed from the primes.
pub fn parse_bcrypt_rsa_private(blob: &[u8]) -> Result<RsaPrivateKey, LicenseError> {
    const RSA2: u32 = 0x3241_5352; // "RSA2" little-endian: plain private (no d)
    const RSA3: u32 = 0x3341_5352; // "RSA3": full private (d present)

    if blob.len() < 24 {
        return Err(LicenseError::RsaBlob("shorter than a 24-byte header".into()));
    }

    let u32le = |at: usize| le_u32(blob, at) as usize;
    let magic = le_u32(blob, 0);
    let cb_public_exp = u32le(8);
    let cb_modulus = u32le(12);
    let cb_prime1 = u32le(16);
    let cb_prime2 = u32le(20);

    let (full, _) = match magic {
        RSA2 => (false, ()),
        RSA3 => (true, ()),
        other => return Err(LicenseError::RsaBlob(format!("unexpected magic {other:#010x}"))),
    };

    let mut at = 24usize;
    let mut take = |n: usize| -> Result<BigUint, LicenseError> {
        let end = at
            .checked_add(n)
            .filter(|&e| e <= blob.len())
            .ok_or_else(|| LicenseError::RsaBlob("field runs past the blob".into()))?;
        let v = BigUint::from_bytes_be(&blob[at..end]);
        at = end;
        Ok(v)
    };

    let e = take(cb_public_exp)?;
    let n = take(cb_modulus)?;
    let p = take(cb_prime1)?;
    let q = take(cb_prime2)?;

    let key = if full {
        // Full-private blob: `d` is present after the primes.
        let d = take(cb_modulus)?;
        RsaPrivateKey::from_components(n, e, d, vec![p, q]).map_err(LicenseError::RsaKeyInvalid)?
    } else {
        // Plain-private blob (the 2048-bit device key's 544-byte form): the primes carry it, and
        // `RsaPrivateKey::from_p_q` recomputes the private exponent. `n` is checked below.
        let key = RsaPrivateKey::from_p_q(p, q, e).map_err(LicenseError::RsaKeyInvalid)?;
        if key.n() != &n {
            return Err(LicenseError::RsaKey("the blob's modulus does not match p·q".into()));
        }
        key
    };

    key.validate().map_err(LicenseError::RsaKeyInvalid)?;
    Ok(key)
}

#[cfg(test)]
mod tests;
