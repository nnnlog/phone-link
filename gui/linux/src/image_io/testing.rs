//! Image fixtures for the tests of the image edges.

use moxcms::ColorProfile;

/// The Display P3 profile, whose pure red lies outside sRGB.
pub fn display_p3() -> Vec<u8> {
    ColorProfile::new_display_p3().encode().expect("the profile encodes")
}
