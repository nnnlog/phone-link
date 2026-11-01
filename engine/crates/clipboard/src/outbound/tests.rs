use super::*;

#[test]
fn the_hash_is_base64_of_sha256() {
    assert_eq!(content_hash(b""), "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=");
    assert_eq!(content_hash(b"hi"), "j0NDRmSPa5bfid2pAcUXaxCm2Dlh3TwayItZstwyeqQ=");
}
