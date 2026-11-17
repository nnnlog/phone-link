//! Picture files for the tests of the desktop picture, and the JPEG the phone gets of them.

use std::path::PathBuf;

use phonelink_wallpaper::PcWallpaper;

/// A folder of test files under the temporary folder, removed when dropped.
pub(super) struct TestDir(PathBuf);

impl TestDir {
    pub(super) fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("phonelink-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub(super) fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    pub(super) fn write(&self, relative: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// A wallpaper package with one picture.
    pub(super) fn package(&self, relative: &str) -> PathBuf {
        let png = encoded_png(&image::RgbImage::from_pixel(4, 2, image::Rgb([1, 2, 3])));
        self.write(&format!("{relative}/contents/images/1920x1080.png"), png);
        self.path(relative)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// `image` as PNG.
pub(super) fn encoded_png(image: &image::RgbImage) -> Vec<u8> {
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    png.into_inner()
}

/// The width and height of the JPEG `picture` holds, and its color at `x`, `y`.
pub(super) fn jpeg_pixel(picture: PcWallpaper, x: u32, y: u32) -> (u32, u32, [u8; 3]) {
    let PcWallpaper::Picture { jpeg } = picture else { panic!("a picture, not {picture:?}") };
    let decoded = image::load_from_memory(&jpeg).unwrap().to_rgb8();
    (decoded.width(), decoded.height(), decoded.get_pixel(x, y).0)
}

/// `color` is `expected` but for JPEG's loss.
pub(super) fn near(color: [u8; 3], expected: [u8; 3]) -> bool {
    color.iter().zip(expected).all(|(got, want)| got.abs_diff(want) <= 6)
}
