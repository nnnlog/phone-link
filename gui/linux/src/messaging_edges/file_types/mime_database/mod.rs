//! The parts of the shared MIME-info database (freedesktop.org) the messaging edges read: its types,
//! aliases and sub-class relations, its file name globs, and each type's own definition.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

use phonelink_core::sync::MutexExt;
use phonelink_messaging::edge::FileFamily;

use super::mac_answers::{DOCUMENT_TYPES, MAC_ALIASES, MacAnswer, mac_answer};

#[cfg(test)]
mod tests;

/// `content_type` lowercased; `None` for a type with parameters (which names no declared type) or
/// without a subtype.
fn normalized(content_type: &str) -> Option<String> {
    let lower = content_type.trim().to_lowercase();
    (!lower.contains(';') && lower.contains('/')).then_some(lower)
}

/// The parts of the shared MIME-info database the messaging edges read.
#[derive(Debug, Default)]
pub struct MimeDatabase {
    /// Lowercase alias → canonical type.
    aliases: HashMap<String, String>,
    /// Lowercase type → its direct parents.
    parents: HashMap<String, Vec<String>>,
    /// Lowercase extension (without the dot) → the type of its heaviest `*.ext` glob (the first of equal
    /// weights), from the most important directory whose `globs2` has one.
    by_extension: HashMap<String, String>,
    /// Lowercase type → the type as the database spells it (`audio/AMR`), for its definition file.
    canonical: HashMap<String, String>,
    /// The `mime` directories, most important first.
    dirs: Vec<PathBuf>,
    /// The preferred extension of each type asked for so far.
    extensions: Mutex<HashMap<String, Option<String>>>,
}

/// `alias canonical` or `type parent` lines (`aliases`, `subclasses`).
fn pairs(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines().filter(|line| !line.starts_with('#')).filter_map(|line| line.split_once(' '))
}

/// The extension of a `*.ext` glob; `None` for any other pattern.
fn glob_extension(glob: &str) -> Option<&str> {
    let extension = glob.strip_prefix("*.")?;
    (!extension.is_empty() && !extension.contains(['*', '?', '[', '.'])).then_some(extension)
}

/// The first `*.ext` glob of a type's definition file.
fn first_glob_extension(definition: &str) -> Option<String> {
    definition
        .match_indices("<glob ")
        .filter_map(|(at, _)| {
            let element = &definition[at..];
            let pattern = element.split_once("pattern=\"")?.1.split_once('"')?.0;
            glob_extension(pattern).map(str::to_lowercase)
        })
        .next()
}

impl MimeDatabase {
    /// The database in `dirs` (each a `mime` directory, most important first); missing files are
    /// skipped.
    pub fn load(dirs: Vec<PathBuf>) -> Self {
        let mut database = Self { dirs, ..Self::default() };
        // Read least important first, so a more important directory's entries replace them.
        for dir in database.dirs.clone().iter().rev() {
            let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
            if let Some(types) = read("types") {
                database.add_types(&types);
            }
            if let Some(aliases) = read("aliases") {
                database.add_aliases(&aliases);
            }
            if let Some(subclasses) = read("subclasses") {
                database.add_subclasses(&subclasses);
            }
            if let Some(globs) = read("globs2") {
                database.add_globs(&globs);
            }
        }

        database
    }

    /// A database was found: it lists its types.
    pub(super) fn is_loaded(&self) -> bool {
        !self.canonical.is_empty()
    }

    fn add_types(&mut self, text: &str) {
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
            self.canonical.insert(line.to_lowercase(), line.to_owned());
        }
    }

    fn add_aliases(&mut self, text: &str) {
        for (alias, canonical) in pairs(text) {
            self.aliases.insert(alias.to_lowercase(), canonical.to_lowercase());
        }
    }

    fn add_subclasses(&mut self, text: &str) {
        let mut read: HashMap<String, Vec<String>> = HashMap::new();
        for (child, parent) in pairs(text) {
            read.entry(child.to_lowercase()).or_default().push(parent.to_lowercase());
        }
        self.parents.extend(read);
    }

    /// `weight:type:glob[:flags]` lines; a heavier glob wins an extension, the first one a tie.
    fn add_globs(&mut self, text: &str) {
        let mut best: HashMap<String, (u32, String)> = HashMap::new();
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            let mut fields = line.split(':');
            let (Some(weight), Some(mime), Some(glob)) = (fields.next(), fields.next(), fields.next()) else {
                continue;
            };
            let (Ok(weight), Some(extension)) = (weight.parse::<u32>(), glob_extension(glob)) else { continue };
            let extension = extension.to_lowercase();
            if best.get(&extension).is_none_or(|(kept, _)| weight > *kept) {
                best.insert(extension, (weight, mime.to_lowercase()));
            }
        }

        self.by_extension.extend(best.into_iter().map(|(extension, (_, mime))| (extension, mime)));
    }

    /// The type a lowercase `mime` names: the type it is an alias of, or itself.
    fn resolve(&self, mime: &str) -> String {
        let alias = MAC_ALIASES.iter().find(|(alias, _)| *alias == mime).map(|(_, target)| *target);
        self.aliases.get(mime).map(String::as_str).or(alias).unwrap_or(mime).to_owned()
    }

    /// The type and every type it is a sub-class of, nearest first.
    fn ancestry(&self, mime: &str) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut order = vec![mime.to_owned()];
        let mut index = 0;
        seen.insert(mime.to_owned());

        while let Some(current) = order.get(index).cloned() {
            for parent in self.parents.get(&current).into_iter().flatten() {
                let parent = self.aliases.get(parent).cloned().unwrap_or_else(|| parent.clone());
                if seen.insert(parent.clone()) {
                    order.push(parent);
                }
            }
            index += 1;
        }

        order
    }

    /// macOS's answer for `mime` (lowercase) as named, else for the type it is an alias of, and the
    /// type it names.
    fn mac_answer(&self, mime: &str) -> (Option<&'static MacAnswer>, String) {
        let resolved = self.resolve(mime);
        (mac_answer(mime).or_else(|| mac_answer(&resolved)), resolved)
    }

    /// The family of `content_type`, tested in this order: vCard, image, audio, video, text, documents
    /// (as named). A type with parameters, no `media/subtype` at all, or one the loaded database neither
    /// knows nor aliases is other.
    pub fn family_of(&self, content_type: &str) -> FileFamily {
        let Some(lower) = normalized(content_type) else { return FileFamily::Other };
        let (mac, mime) = self.mac_answer(&lower);
        if let Some(mac) = mac {
            return mac.family;
        }
        if self.is_loaded() && !self.canonical.contains_key(&mime) {
            return FileFamily::Other;
        }
        let ancestry = self.ancestry(&mime);
        let any = |test: &dyn Fn(&str) -> bool| ancestry.iter().any(|t| test(t));
        if any(&|t| t == "text/vcard") {
            FileFamily::VCard
        } else if any(&|t| t.starts_with("image/")) {
            FileFamily::Image
        } else if any(&|t| t.starts_with("audio/")) {
            FileFamily::Audio
        } else if any(&|t| t.starts_with("video/")) {
            FileFamily::Video
        } else if any(&|t| t.starts_with("text/")) {
            FileFamily::Text
        } else if DOCUMENT_TYPES.contains(&lower.as_str()) {
            FileFamily::Document
        } else {
            FileFamily::Other
        }
    }

    /// The preferred extension of `content_type` with its leading dot: macOS's, else the first `*.ext`
    /// glob of its definition file.
    pub fn extension_of(&self, content_type: &str) -> Option<String> {
        let lower = normalized(content_type)?;
        let (mac, mime) = self.mac_answer(&lower);
        if let Some(mac) = mac {
            return mac.extension.map(str::to_owned);
        }
        let spelled = self.canonical.get(&mime)?;
        let mut cache = self.extensions.lock_or_recover();
        if let Some(known) = cache.get(&mime) {
            return known.clone();
        }
        // Newer databases name the definition files in lowercase (`audio/amr.xml` for `audio/AMR`).
        let extension = self
            .dirs
            .iter()
            .find_map(|dir| {
                [spelled, &mime]
                    .into_iter()
                    .find_map(|name| std::fs::read_to_string(dir.join(format!("{name}.xml"))).ok())
            })
            .and_then(|definition| first_glob_extension(&definition))
            .map(|extension| format!(".{extension}"));
        cache.insert(mime, extension.clone());
        extension
    }

    /// The MIME type a file name's extension names; `None` when it has none the database knows.
    pub fn type_of_file_name(&self, name: &str) -> Option<String> {
        let (_, extension) = name.rsplit_once('.')?;
        self.by_extension.get(&extension.to_lowercase()).cloned()
    }
}
