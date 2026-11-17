//! GNOME's slideshow and timed backgrounds (the `<background>` XML): their slides, and the picture
//! they show at a time.

use std::path::{Path, PathBuf};

use super::plasma::package_choice;

#[cfg(test)]
mod tests;

/// Why a slideshow was not read, or names no picture.
#[derive(Debug, thiserror::Error)]
pub(super) enum SlideshowError {
    #[error("{0}")]
    Xml(#[from] roxmltree::Error),
    #[error("the duration {0:?}")]
    Duration(String),
    #[error("the start {field} {value:?}")]
    StartField { field: &'static str, value: String },
    #[error("the start time is no local time")]
    StartTime,
    #[error("a <static> without its <file>")]
    NoFile,
    #[error("a <static> without its picture")]
    NoPicture,
    #[error("a <{element}> without its <{child}>")]
    MissingChild { element: String, child: &'static str },
    #[error("the slideshow lasts no time")]
    NoDuration,
    #[error("the slideshow has no slides")]
    NoSlides,
    /// A slideshow's slide is a slideshow itself.
    #[error("a slideshow names another slideshow")]
    Nested,
}

/// One part of a GNOME background slideshow: a picture shown for a while, or a cross-fade from one
/// picture to the next.
#[derive(Debug, Clone, PartialEq)]
enum Slide {
    Static { seconds: f64, file: PathBuf },
    Transition { seconds: f64, from: PathBuf, to: PathBuf },
}

impl Slide {
    fn seconds(&self) -> f64 {
        match self {
            Self::Static { seconds, .. } | Self::Transition { seconds, .. } => *seconds,
        }
    }
}

/// A GNOME background slideshow (the `<background>` XML of GNOME's slideshow and timed backgrounds):
/// its slides, which repeat from its start time on.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Slideshow {
    /// Unix time; the epoch when the file names none.
    start: i64,
    slides: Vec<Slide>,
}

impl Slideshow {
    /// Reads `xml`; a picture's relative path is taken from `folder`. A picture given in several sizes
    /// is the one a Plasma package would send.
    pub(super) fn parse(xml: &str, folder: &Path) -> Result<Self, SlideshowError> {
        let options = roxmltree::ParsingOptions { allow_dtd: true, ..roxmltree::ParsingOptions::default() };
        let document = roxmltree::Document::parse_with_options(xml, options)?;
        let path = |text: &str| folder.join(text);
        let seconds = |node: roxmltree::Node<'_, '_>| -> Result<f64, SlideshowError> {
            let duration = xml_text(node, "duration")?;
            let seconds = duration.parse::<f64>().ok().filter(|seconds| seconds.is_finite() && *seconds >= 0.0);
            seconds.ok_or(SlideshowError::Duration(duration))
        };

        let mut start = 0;
        let mut slides = Vec::new();
        for node in document.root_element().children().filter(roxmltree::Node::is_element) {
            match node.tag_name().name() {
                "starttime" => {
                    let field = |name: &'static str| -> Result<i32, SlideshowError> {
                        let value = xml_text(node, name)?;
                        value.parse().map_err(|_| SlideshowError::StartField { field: name, value })
                    };
                    let (year, month, day) = (field("year")?, field("month")?, field("day")?);
                    let (hour, minute, second) = (field("hour")?, field("minute")?, field("second")?);
                    start = local_time(year, month, day, hour, minute, second).ok_or(SlideshowError::StartTime)?;
                }
                "static" => {
                    let file = xml_child(node, "file").ok_or(SlideshowError::NoFile)?;
                    let sizes: Vec<((u32, u32), String)> = file
                        .children()
                        .filter(|size| size.has_tag_name("size"))
                        .filter_map(|size| {
                            let width = size.attribute("width")?.parse().ok()?;
                            let height = size.attribute("height")?.parse().ok()?;
                            Some(((width, height), size.text()?.trim().to_owned()))
                        })
                        .collect();
                    let chosen = package_choice(&sizes.iter().map(|(size, _)| *size).collect::<Vec<_>>());
                    let file = match chosen {
                        Some(chosen) => sizes.into_iter().find(|(size, _)| *size == chosen).map(|(_, name)| name),
                        None => file.text().map(|name| name.trim().to_owned()),
                    }
                    .filter(|name| !name.is_empty())
                    .ok_or(SlideshowError::NoPicture)?;
                    slides.push(Slide::Static { seconds: seconds(node)?, file: path(&file) });
                }
                "transition" => {
                    let (from, to) = (xml_text(node, "from")?, xml_text(node, "to")?);
                    slides.push(Slide::Transition { seconds: seconds(node)?, from: path(&from), to: path(&to) });
                }
                _ => {}
            }
        }

        Ok(Self { start, slides })
    }

    /// The picture it shows at `now` (Unix time): a static slide's, or the nearer one of a cross-fade.
    pub(super) fn picture_at(&self, now: i64) -> Result<&Path, SlideshowError> {
        let total: f64 = self.slides.iter().map(Slide::seconds).sum();
        if total <= 0.0 {
            return Err(SlideshowError::NoDuration);
        }
        let mut at = (now - self.start) as f64 % total;
        if at < 0.0 {
            at += total;
        }

        for slide in &self.slides {
            if at < slide.seconds() {
                return Ok(match slide {
                    Slide::Static { file, .. } => file,
                    Slide::Transition { seconds, from, to } => {
                        if at < seconds / 2.0 {
                            from
                        } else {
                            to
                        }
                    }
                });
            }
            at -= slide.seconds();
        }

        // Rounding left the time past the last slide: it ends the round.
        match self.slides.last() {
            Some(Slide::Static { file, .. }) => Ok(file),
            Some(Slide::Transition { to, .. }) => Ok(to),
            None => Err(SlideshowError::NoSlides),
        }
    }
}

/// `node`'s first child element `name`.
fn xml_child<'a, 'input>(node: roxmltree::Node<'a, 'input>, name: &str) -> Option<roxmltree::Node<'a, 'input>> {
    node.children().find(|child| child.has_tag_name(name))
}

/// The trimmed text of `node`'s child element `name`.
fn xml_text(node: roxmltree::Node<'_, '_>, name: &'static str) -> Result<String, SlideshowError> {
    xml_child(node, name)
        .and_then(|child| child.text())
        .map(|text| text.trim().to_owned())
        .ok_or_else(|| SlideshowError::MissingChild { element: node.tag_name().name().to_owned(), child: name })
}

/// The Unix time of a local date and time, as `mktime` takes it (with the time zone's rules of that
/// date, as GNOME reads a slideshow's start).
#[expect(unsafe_code, reason = "mktime has no safe wrapper in the dependencies")]
fn local_time(year: i32, month: i32, day: i32, hour: i32, minute: i32, second: i32) -> Option<i64> {
    // SAFETY: `tm` is plain data; zeroed, its time-zone name is a null pointer, which `mktime` does not
    // read.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month - 1;
    tm.tm_mday = day;
    tm.tm_hour = hour;
    tm.tm_min = minute;
    tm.tm_sec = second;
    tm.tm_isdst = -1;
    // SAFETY: `tm` is a valid, exclusively borrowed `struct tm` for the call.
    let time = unsafe { libc::mktime(&mut tm) };
    (time != -1).then_some(time)
}
