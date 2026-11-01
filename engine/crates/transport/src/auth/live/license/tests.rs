use super::*;
use aes::cipher::{BlockEncryptMut, KeyIvInit};

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;

#[test]
fn kdf_known_answers() {
    let key: Vec<u8> = (0u8..32).collect();
    let ctx = [0xAAu8; 32];

    assert_eq!(
        hex(&sp800_108(&key, KDF_LABEL, &ctx, 32)),
        "59e13a417f67f819173aba78d9849f26cbfa27b0a5d42de46b449adfad1da028"
    );
    assert_eq!(
        hex(&sp800_108(&key, KDF_LABEL, &ctx, 48)),
        "1143335f585848f1cd2bec8b878c7e8b616698c5dc550af79cd7ca7997f1133e9c82335dbbc2fae56d3577b612b5b770"
    );
}

#[test]
fn response_decrypt_roundtrip() {
    let key = [7u8; 32];
    let iv = [9u8; 16];
    let plaintext = vec![b'x'; 8192];

    let mut buf = plaintext.clone();
    buf.resize(plaintext.len() + 16, 0);
    let ct = Aes256CbcEnc::new(&key.into(), &iv.into())
        .encrypt_padded_mut::<Pkcs7>(&mut buf, plaintext.len())
        .unwrap()
        .to_vec();

    let mut cipher_value = iv.to_vec();
    cipher_value.extend_from_slice(&ct);

    assert_eq!(decrypt_response(&cipher_value, &key).unwrap(), plaintext);
}

#[test]
fn response_decrypt_rejects_short() {
    assert!(matches!(decrypt_response(&[0; 15], &[0; 32]).unwrap_err(), LicenseError::ShortPayload));
}

#[test]
fn clep_key_extraction_matches_reference() {
    let schedule: [u32; 58] = std::array::from_fn(|i| (i as u32).wrapping_mul(0x0101_0101));
    assert_eq!(hex(&clep_aes_key(&schedule)), "81caa938320503d457d3f35cdbbb1f00");
}

#[test]
fn clep_block_decrypts_against_its_schedule() {
    // Build a block whose schedule yields a known AES key, then AES-128-CBC/zero-IV encrypt a
    // chosen plaintext into key_data, and confirm the block decrypts back to it.
    let schedule: [u32; 58] = std::array::from_fn(|i| (i as u32).wrapping_mul(0x0001_2345));
    let key = clep_aes_key(&schedule);

    let plaintext: Vec<u8> = (0..HMAC_STATE_DATA_LEN as u8).collect();
    let mut buf = plaintext.clone();
    let ct = Aes128CbcEnc::new(&key.into(), &[0u8; 16].into())
        .encrypt_padded_mut::<NoPadding>(&mut buf, plaintext.len())
        .unwrap()
        .to_vec();

    let mut block = [0u8; 4096];
    block[0..4].copy_from_slice(&4u32.to_le_bytes());
    block[4..4 + HMAC_STATE_DATA_LEN].copy_from_slice(&ct);
    for (i, w) in schedule.iter().enumerate() {
        let at = 4 + HMAC_STATE_DATA_LEN + i * 4;
        block[at..at + 4].copy_from_slice(&w.to_le_bytes());
    }

    assert_eq!(decrypt_clep_block(&block, HMAC_STATE_DATA_LEN).unwrap(), plaintext);
}

// A fixed 1024-bit RSA key (hex, big-endian) so the blob tests are deterministic and need no
// key generation or RNG.
const N: &str = "bdd58297feacbd7cc8ab2aa39bf0b47ce5e41a843db257a00647be74d537e08890d4c4e2a1d9d541b11c9dedf883d8787c9ad7933e1c52b9c0f7471f1fcd02f88bc2d6e7ae8d76cb268195ee0f8252e7d2497a7cc980e19f57880433700c076e28618d3bf6f046e6938d8c0fa974ee8b8d169fe1219b44001e3a0ebbcbcbe6c5";
const E: &str = "010001";
const D: &str = "0e0b5f3440c10c1e37119dffc2bb85fbb05d9c7b0224b76e7d354202f4c8f6eb106a6a328fc811bdfa404abfab615dc127720ad6eae9a5ec5c9fc8610c8240fa7c9ded26566d2d9b4da7542ed7f0f222f8713c9a9dd2fa76e180b75b84aff6ecd04e1f1e118ea74a7cefa746c949d3259f8649b85a314f6e16d622de43e71455";
const P: &str = "d804209783b23ed9ae5a440cb9771917937e92c0539e4c7d94fbeeb996486f4f2be5e9d6f184f6418a67a3378d6f61fc43874a22dd934493fb334ba5e3d6d3cf";
const Q: &str = "e0f8c013d9f3b47039b397f77be147456008c86c780a1fc73ba2d0db5e182d5a41ac81f11d08f016e3b7846b3ad035bc24319293688872019b478d17ef623d2b";

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// `c = m^e mod n`, `m2 = c^d mod n`; a correct `d` recovers `m`.
fn rsa_roundtrips(key: &RsaPrivateKey) -> bool {
    use rsa::traits::PrivateKeyParts;
    let m = BigUint::from(42u32);
    let c = m.modpow(key.e(), key.n());
    let back = c.modpow(key.d(), key.n());
    back == m
}

fn rsa_blob(magic: u32, with_d: bool) -> Vec<u8> {
    let (n, e, d, p, q) = (unhex(N), unhex(E), unhex(D), unhex(P), unhex(Q));
    let mut blob = Vec::new();
    blob.extend_from_slice(&magic.to_le_bytes());
    blob.extend_from_slice(&((n.len() * 8) as u32).to_le_bytes());
    blob.extend_from_slice(&(e.len() as u32).to_le_bytes());
    blob.extend_from_slice(&(n.len() as u32).to_le_bytes());
    blob.extend_from_slice(&(p.len() as u32).to_le_bytes());
    blob.extend_from_slice(&(q.len() as u32).to_le_bytes());

    blob.extend_from_slice(&e);
    blob.extend_from_slice(&n);
    blob.extend_from_slice(&p);
    blob.extend_from_slice(&q);
    if with_d {
        blob.extend_from_slice(&d);
    }

    blob
}

#[test]
fn bcrypt_rsa_full_blob_roundtrip() {
    let parsed = parse_bcrypt_rsa_private(&rsa_blob(0x3341_5352, true)).unwrap();
    assert_eq!(parsed.n(), &BigUint::from_bytes_be(&unhex(N)));
    assert!(rsa_roundtrips(&parsed));
}

#[test]
fn bcrypt_rsa_plain_blob_recomputes_d() {
    let parsed = parse_bcrypt_rsa_private(&rsa_blob(0x3241_5352, false)).unwrap();
    assert_eq!(parsed.n(), &BigUint::from_bytes_be(&unhex(N)));
    assert!(rsa_roundtrips(&parsed));
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
