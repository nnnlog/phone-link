use super::*;

#[test]
fn decodes_the_spec_envelope_vector() {
    let bytes = read_input(
            "01 27000000 02000000 06000000 5F726F757465 02000000 2F74 0A000000 5F72657175657374 4964 01000000 37 0C00000000000000 0A0A 0A0161 1205 080C 6A0162",
            false,
        )
        .unwrap();

    let j = decode(Layer::Envelope, &bytes).unwrap();

    assert_eq!(j["headers"]["_route"], "/t");
    assert_eq!(j["payload"]["valueset"]["a"]["String"], "b");
}
