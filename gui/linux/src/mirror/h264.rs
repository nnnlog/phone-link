//! H.264 access units decoded with `OpenH264`, for the mirrored windows and the phone's camera.

use std::borrow::Cow;

use openh264::decoder::{DecodedYUV, Decoder};

const H264_NAL_SPS: u8 = 7;
const OPENH264_MAX_LEVEL: u8 = 51;

// OpenH264 refuses the phone's sequence parameter set at level 6.0 with dsNoParamSets and takes it
// at 5.1; the level only bounds decoder resources, so it is lowered in place.
fn with_supported_level(data: &[u8]) -> Cow<'_, [u8]> {
    let levels: Vec<usize> = data
        .windows(7)
        .enumerate()
        .filter(|(_, w)| w[..3] == [0, 0, 1] && w[3] & 0x1F == H264_NAL_SPS && w[6] > OPENH264_MAX_LEVEL + 1)
        .map(|(i, _)| i + 6)
        .collect();
    if levels.is_empty() {
        return Cow::Borrowed(data);
    }

    let mut patched = data.to_vec();
    for index in levels {
        patched[index] = OPENH264_MAX_LEVEL;
    }
    Cow::Owned(patched)
}

/// Decodes one access unit, and converts its picture with `convert` when it completes one.
///
/// Each NAL unit is decoded on its own: `OpenH264` keeps the phone's parameter sets only then.
pub(crate) fn decode_access_unit<T>(
    decoder: &mut Decoder,
    data: &[u8],
    convert: impl FnOnce(&DecodedYUV<'_>) -> T,
) -> Result<Option<T>, openh264::Error> {
    let data = with_supported_level(data);
    let units: Vec<&[u8]> = openh264::nal_units(&data).collect();
    let Some((last, leading)) = units.split_last() else { return Ok(None) };
    for unit in leading {
        decoder.decode(unit)?;
    }
    Ok(decoder.decode(last)?.map(|picture| convert(&picture)))
}
