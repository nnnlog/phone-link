//! The camera's frames: 1280 × 720 I420, black, or a decoded picture fitted to that size.

use openh264::formats::YUVSource;

pub(super) const WIDTH: usize = 1280;
pub(super) const HEIGHT: usize = 720;
pub(super) const FRAME_BYTES: usize = WIDTH * HEIGHT * 3 / 2;

pub(super) fn black() -> Vec<u8> {
    let mut frame = vec![16; WIDTH * HEIGHT];
    frame.resize(FRAME_BYTES, 128);
    frame
}

/// Copies the rows of a plane `width` bytes wide and `rows` rows high (`source`, `source_stride` bytes a
/// row) to the top left of `target` (`target_stride` bytes a row, `target_rows` rows): what does not fit is
/// cut, and the rest of `target` stays as it is. A source shorter than its rows ends the copy.
pub(super) fn copy_plane(
    target: &mut [u8],
    (target_stride, target_rows): (usize, usize),
    source: &[u8],
    source_stride: usize,
    (width, rows): (usize, usize),
) {
    let width = width.min(target_stride);
    for row in 0..rows.min(target_rows) {
        let start = row * source_stride;
        let Some(from) = source.get(start..start + width) else { return };
        target[row * target_stride..row * target_stride + width].copy_from_slice(from);
    }
}

/// `picture` in a camera frame: each plane copied in row by row from the top left, a larger picture cut
/// to the frame and a smaller one padded with black (luma 16, chroma 128).
pub(super) fn frame_from(picture: &impl YUVSource) -> Vec<u8> {
    let (width, height) = picture.dimensions();
    let (y_stride, u_stride, v_stride) = picture.strides();
    let chroma = (width.div_ceil(2), height.div_ceil(2));

    let mut frame = black();
    let (luma, chroma_planes) = frame.split_at_mut(WIDTH * HEIGHT);
    let (u_plane, v_plane) = chroma_planes.split_at_mut(WIDTH * HEIGHT / 4);
    copy_plane(luma, (WIDTH, HEIGHT), picture.y(), y_stride, (width, height));
    copy_plane(u_plane, (WIDTH / 2, HEIGHT / 2), picture.u(), u_stride, chroma);
    copy_plane(v_plane, (WIDTH / 2, HEIGHT / 2), picture.v(), v_stride, chroma);
    frame
}
